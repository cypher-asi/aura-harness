//! Plan files and the tool that writes them.
//!
//! A plan lives in `.aura/plans/<slug>_<id>.plan.md` inside the workspace:
//! YAML frontmatter (`name`, `overview`, `todos`) followed by a Markdown
//! body. `create_plan` writes one (in plan mode the agent can't edit code,
//! but plans are Markdown). While a plan is built, its todos are the
//! agent's `todo_write` list: [`todo_items`] seeds the list from the plan
//! and [`sync_todo_list`] writes the list's progress back to the file.

use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::todo::TodoItem;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path};

pub use crate::todo::TodoStatus;

/// Workspace-relative directory plans are written to.
pub const PLANS_DIR: &str = ".aura/plans";
/// File name suffix every plan carries.
pub const PLAN_SUFFIX: &str = ".plan.md";
/// Name of the tool that writes a plan.
pub const CREATE_PLAN_TOOL_NAME: &str = "create_plan";

/// Start of the first output line of `create_plan`, followed by the plan
/// path. See [`plan_path_from_output`].
const SAVED_PREFIX: &str = "Plan saved: ";

/// One step of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanTodo {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub status: TodoStatus,
}

/// Frontmatter of a plan file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanMeta {
    pub name: String,
    #[serde(default)]
    pub overview: String,
    #[serde(default)]
    pub todos: Vec<PlanTodo>,
}

/// A parsed plan file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanFile {
    pub meta: PlanMeta,
    pub body: String,
}

impl PlanFile {
    /// Parse `---`-delimited YAML frontmatter followed by the body.
    ///
    /// # Errors
    /// When the frontmatter is missing or isn't valid plan YAML.
    pub fn parse(text: &str) -> Result<Self, String> {
        let rest = text
            .strip_prefix("---")
            .ok_or("plan file has no frontmatter")?;
        let (yaml, body) = rest
            .split_once("\n---")
            .ok_or("plan frontmatter is not closed")?;
        let meta: PlanMeta =
            serde_yaml::from_str(yaml).map_err(|e| format!("invalid plan frontmatter: {e}"))?;
        let body = body.strip_prefix('\n').unwrap_or(body);
        Ok(Self {
            meta,
            body: body.trim_start_matches('\n').to_string(),
        })
    }

    /// Read and parse the plan at `path`.
    ///
    /// # Errors
    /// When the file can't be read or parsed.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text)
    }

    /// The file's text: frontmatter, blank line, body.
    #[must_use]
    pub fn render(&self) -> String {
        let yaml = serde_yaml::to_string(&self.meta).unwrap_or_default();
        format!("---\n{yaml}---\n\n{}", self.body)
    }

    /// `(closed, total)` todo counts.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let closed = self
            .meta
            .todos
            .iter()
            .filter(|t| t.status.is_closed())
            .count();
        (closed, self.meta.todos.len())
    }
}

/// The plan path a successful `create_plan` reported.
#[must_use]
pub fn plan_path_from_output(output: &str) -> Option<&str> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(SAVED_PREFIX).map(str::trim))
}

/// Whether `path` is a workspace-relative `.plan.md` file under
/// [`PLANS_DIR`], without `..` or absolute components.
#[must_use]
pub fn is_plan_path(path: &str) -> bool {
    let p = Path::new(path);
    p.starts_with(PLANS_DIR)
        && p.components().all(|c| matches!(c, Component::Normal(_)))
        && path.ends_with(PLAN_SUFFIX)
}

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
fn slug(name: &str) -> String {
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
/// of todos whose id it already has.
pub fn create_plan(sandbox: &Sandbox, args: &serde_json::Value) -> Result<ToolResult, ToolError> {
    let name = required_str(args, "name")?.trim().to_string();
    let overview = str_arg(args, "overview")
        .unwrap_or_default()
        .trim()
        .to_string();
    let body = required_str(args, "plan")?.to_string();
    let todos = todo_args(args)?;

    let (path, previous) = match str_arg(args, "path").filter(|p| !p.trim().is_empty()) {
        Some(path) => {
            check_plan_path(path)?;
            (path.to_string(), Some(read_plan(sandbox, path)?))
        }
        None => {
            let id = uuid::Uuid::new_v4().simple().to_string();
            let path = format!("{PLANS_DIR}/{}_{}{PLAN_SUFFIX}", slug(&name), &id[..8]);
            (path, None)
        }
    };

    let mut planned = Vec::with_capacity(todos.len());
    for todo in todos {
        let content = todo
            .content
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| {
                ToolError::InvalidArguments(format!("todo '{}' has no content", todo.id))
            })?;
        let status = todo.status.unwrap_or_else(|| {
            previous
                .as_ref()
                .and_then(|p| p.meta.todos.iter().find(|t| t.id == todo.id))
                .map_or(TodoStatus::Pending, |t| t.status)
        });
        planned.push(PlanTodo {
            id: todo.id,
            content,
            status,
        });
    }

    let plan = PlanFile {
        meta: PlanMeta {
            name,
            overview,
            todos: planned,
        },
        body,
    };
    write_plan(sandbox, &path, &plan)?;
    let verb = if previous.is_some() {
        "Updated"
    } else {
        "Created"
    };
    let output = format!(
        "{SAVED_PREFIX}{path}\n{verb} plan '{}' with {} todos:\n{}",
        plan.meta.name,
        plan.meta.todos.len(),
        todo_lines(&plan)
    );
    Ok(ToolResult::success(CREATE_PLAN_TOOL_NAME, output).with_metadata("plan_path", path))
}

/// The plan's todos as a `todo_write` list, to start building it with.
#[must_use]
pub fn todo_items(plan: &PlanFile) -> Vec<TodoItem> {
    plan.meta
        .todos
        .iter()
        .map(|todo| TodoItem {
            id: todo.id.clone(),
            content: todo.content.clone(),
            status: todo.status,
        })
        .collect()
}

/// Make the todos of the plan file at `path` the agent's `todo_write`
/// list `items`, keeping the plan's body. Returns whether the file
/// changed.
///
/// # Errors
/// When the plan can't be read, parsed or written.
pub fn sync_todo_list(path: &Path, items: &[TodoItem]) -> Result<bool, String> {
    let mut plan = PlanFile::load(path)?;
    let todos: Vec<PlanTodo> = items
        .iter()
        .map(|item| PlanTodo {
            id: item.id.clone(),
            content: item.content.clone(),
            status: item.status,
        })
        .collect();
    if plan.meta.todos == todos {
        return Ok(false);
    }
    plan.meta.todos = todos;
    fs::write(path, plan.render()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
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
            input_schema: serde_json::json!({
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
                        "description": "The plan in Markdown: approach, files to change, \
                                        key snippets, risks"
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
            }),
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
        crate::fs_tools::spawn_blocking_tool(move || create_plan(&sandbox, &args)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn sandbox() -> (Sandbox, TempDir) {
        let dir = TempDir::new().unwrap();
        (Sandbox::new(dir.path()).unwrap(), dir)
    }

    fn output(result: &ToolResult) -> String {
        String::from_utf8_lossy(&result.stdout).to_string()
    }

    fn new_plan(sandbox: &Sandbox) -> String {
        let result = create_plan(
            sandbox,
            &json!({
                "name": "Add Parser!",
                "overview": "Parse things",
                "plan": "# Steps\n\nDo it.",
                "todos": [
                    {"id": "a", "content": "first"},
                    {"id": "b", "content": "second"}
                ]
            }),
        )
        .unwrap();
        plan_path_from_output(&output(&result)).unwrap().to_string()
    }

    #[test]
    fn create_plan_writes_frontmatter_and_body() {
        let (sandbox, dir) = sandbox();
        let path = new_plan(&sandbox);
        assert!(path.starts_with(".aura/plans/add-parser_"), "{path}");
        assert!(path.ends_with(PLAN_SUFFIX));

        let plan = PlanFile::load(&dir.path().join(&path)).unwrap();
        assert_eq!(plan.meta.name, "Add Parser!");
        assert_eq!(plan.meta.overview, "Parse things");
        assert_eq!(plan.body, "# Steps\n\nDo it.");
        assert_eq!(plan.meta.todos.len(), 2);
        assert!(plan
            .meta
            .todos
            .iter()
            .all(|t| t.status == TodoStatus::Pending));
        assert_eq!(PlanFile::parse(&plan.render()).unwrap(), plan);
    }

    #[test]
    fn rewriting_a_plan_keeps_todo_status_by_id() {
        let (sandbox, dir) = sandbox();
        let path = new_plan(&sandbox);
        let file = dir.path().join(&path);
        let mut items = todo_items(&PlanFile::load(&file).unwrap());
        items[0].status = TodoStatus::Completed;
        sync_todo_list(&file, &items).unwrap();
        create_plan(
            &sandbox,
            &json!({
                "name": "Add Parser",
                "overview": "Revised",
                "plan": "new body",
                "todos": [{"id": "a", "content": "first again"}, {"id": "c", "content": "third"}],
                "path": path
            }),
        )
        .unwrap();
        let plan = PlanFile::load(&dir.path().join(&path)).unwrap();
        let statuses: Vec<_> = plan
            .meta
            .todos
            .iter()
            .map(|t| (t.id.as_str(), t.status))
            .collect();
        assert_eq!(
            statuses,
            [("a", TodoStatus::Completed), ("c", TodoStatus::Pending)]
        );
        assert_eq!(plan.body, "new body");
    }

    #[test]
    fn the_agents_todo_list_is_synced_into_the_plan() {
        let (sandbox, dir) = sandbox();
        let path = new_plan(&sandbox);
        let file = dir.path().join(&path);
        let mut items = todo_items(&PlanFile::load(&file).unwrap());
        assert_eq!(items.len(), 2);
        items[0].status = TodoStatus::Completed;
        items[1].status = TodoStatus::InProgress;
        items.push(TodoItem {
            id: "c".into(),
            content: "third".into(),
            status: TodoStatus::Pending,
        });

        assert!(sync_todo_list(&file, &items).unwrap());
        let plan = PlanFile::load(&file).unwrap();
        assert_eq!(plan.progress(), (1, 3));
        assert_eq!(plan.meta.todos[1].status, TodoStatus::InProgress);
        assert_eq!(plan.meta.todos[2].content, "third");
        assert_eq!(plan.body, "# Steps\n\nDo it.");
        assert!(!sync_todo_list(&file, &items).unwrap(), "unchanged");
    }

    #[test]
    fn plan_paths_must_stay_under_the_plans_dir() {
        let (sandbox, _dir) = sandbox();
        for bad in [
            "src/main.plan.md",
            ".aura/plans/../../src/lib.plan.md",
            ".aura/plans/notes.md",
            "/etc/x.plan.md",
        ] {
            let args = json!({"name": "x", "plan": "y", "todos": [], "path": bad});
            let err = create_plan(&sandbox, &args).unwrap_err();
            assert!(
                matches!(err, ToolError::InvalidArguments(_)),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(slug("Add a /plan mode!"), "add-a-plan-mode");
        assert_eq!(slug("???"), "plan");
    }
}
