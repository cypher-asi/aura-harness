//! Plan files and the tool that writes them.
//!
//! A plan lives in `.aura/plans/<slug>_<id>.plan.md` inside the workspace.
//! `create_plan` writes one (in plan mode the agent can't edit code, but
//! plans are Markdown). The file has four parts, in order:
//!
//! 1. YAML frontmatter between `---` lines: `name`, `overview` and `todos`
//!    (id, content, status, assignee). This is the source of truth; code
//!    reads the plan's metadata and todos from here only.
//! 2. A generated, human-readable header: `# <name>`, the overview
//!    paragraph and a `## Todos` checklist (`- [x] ...`). It exists because
//!    the file is often read raw, where the frontmatter alone says little.
//!    It is rebuilt from the frontmatter on every write, so it never needs
//!    hand edits and is never parsed.
//! 3. The [`HEADER_END`] marker, an HTML comment that Markdown renderers
//!    hide, separating generated text from authored text.
//! 4. The body the plan's author wrote, tidied by [`body::normalize_body`].
//!
//! Files written before the header existed have no marker; their body is
//! everything after the frontmatter, and the next write upgrades them.
//!
//! `create_plan` also stamps where the plan came from (see [`PlanOrigin`]):
//! when it was created and last rewritten, the author, its model and the
//! machine. Each is optional, so plans from before them still parse and
//! render without them.
//!
//! While a plan is built, its todos are the agent's `todo_write` list:
//! [`todo_items`] seeds the list from the plan and [`sync_todo_list`]
//! writes the list's progress back to the frontmatter and the checklist.

mod body;
mod create;
mod header;
mod origin;

pub use create::{create_plan, create_plan_from, CreatePlanTool};
pub use origin::{machine_name, PlanOrigin};

use crate::todo::TodoItem;
use header::{after_header, generated_header};
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

/// Line that ends the generated header of a plan file; the authored body
/// follows it. The terminal UI looks for it too, so keep the two in step.
pub const HEADER_END: &str = "<!-- end of generated header: edit below -->";

/// One step of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanTodo {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub status: TodoStatus,
    /// The agent building the plan gave the todo to, if any
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
}

/// Frontmatter of a plan file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanMeta {
    pub name: String,
    #[serde(default)]
    pub overview: String,
    /// When the plan was first written (RFC 3339, UTC); none on plans
    /// from before it was recorded
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// When `create_plan` last rewrote it (RFC 3339, UTC); todo progress
    /// alone does not count
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Id (hex) of the agent that wrote it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Model the author ran on when it last wrote the plan
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Where it was written: a cloud machine's hostname, or `local`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default)]
    pub todos: Vec<PlanTodo>,
}

/// A parsed plan file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanFile {
    pub meta: PlanMeta,
    /// Markdown the plan's author wrote, without the generated header and
    /// without a trailing newline ([`PlanFile::render`] adds one).
    pub body: String,
}

impl PlanFile {
    /// Parse `---`-delimited YAML frontmatter followed by the body.
    ///
    /// The generated header is skipped, never read: metadata and todos
    /// come from the frontmatter only. Without a [`HEADER_END`] marker
    /// (files from before the header) the body is all text after the
    /// frontmatter.
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
        let body = body.trim_start_matches('\n');
        let body = match after_header(&meta, body) {
            Some(authored) => authored.trim_start_matches('\n'),
            None => body,
        };
        let body = body.trim_end_matches('\n');
        Ok(Self {
            meta,
            body: body.to_string(),
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

    /// The file's text: frontmatter, generated header, [`HEADER_END`],
    /// body, ending in exactly one newline.
    #[must_use]
    pub fn render(&self) -> String {
        let yaml = serde_yaml::to_string(&self.meta).unwrap_or_default();
        let mut out = format!("---\n{yaml}---\n\n{}", generated_header(&self.meta));
        let body = self.body.trim_end_matches('\n');
        if !body.is_empty() {
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
        out
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
            assignee: todo.assignee.clone(),
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
            assignee: item.assignee.clone(),
        })
        .collect();
    if plan.meta.todos == todos {
        return Ok(false);
    }
    plan.meta.todos = todos;
    fs::write(path, plan.render()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod origin_tests;
#[cfg(test)]
mod tests;
