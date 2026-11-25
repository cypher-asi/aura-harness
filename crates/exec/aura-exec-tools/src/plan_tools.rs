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
//! 4. The body the plan's author wrote, tidied by [`normalize_body`].
//!
//! Files written before the header existed have no marker; their body is
//! everything after the frontmatter, and the next write upgrades them.
//!
//! While a plan is built, its todos are the agent's `todo_write` list:
//! [`todo_items`] seeds the list from the plan and [`sync_todo_list`]
//! writes the list's progress back to the frontmatter and the checklist.

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

/// `# name`, the overview, the `## Todos` checklist and the
/// [`HEADER_END`] line, each block followed by a blank line except the
/// marker.
fn generated_header(meta: &PlanMeta) -> String {
    // A heading is one line; a name with newlines would spill into the body.
    let title = meta.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = format!("# {title}\n\n");
    let overview = meta.overview.trim();
    if !overview.is_empty() {
        out.push_str(overview);
        out.push_str("\n\n");
    }
    if !meta.todos.is_empty() {
        out.push_str("## Todos\n\n");
        for todo in &meta.todos {
            out.push_str(&checklist_item(todo));
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(HEADER_END);
    out.push('\n');
    out
}

/// `- [x] content (@assignee)`, with continuation lines indented so a
/// multi-line todo stays one list item.
fn checklist_item(todo: &PlanTodo) -> String {
    let mut out = format!("- {} ", todo.status.checkbox());
    for (index, line) in todo.content.trim().lines().enumerate() {
        let line = line.trim_end();
        if index > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str("  ");
            }
        }
        out.push_str(line);
    }
    if let Some(assignee) = todo.assignee.as_deref().filter(|a| !a.trim().is_empty()) {
        out.push_str(&format!(" (@{})", assignee.trim()));
    }
    out
}

/// The text after the generated header in `text` (what follows the
/// frontmatter), or `None` when there is no header.
///
/// The header `meta` renders to is matched first, so a marker line inside
/// the overview or a todo can't cut the header short; failing that (the
/// header was edited by hand) the first marker line ends it.
fn after_header<'a>(meta: &PlanMeta, text: &'a str) -> Option<&'a str> {
    if let Some(rest) = text.strip_prefix(generated_header(meta).as_str()) {
        return Some(rest);
    }
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end() == HEADER_END {
            return text.get(offset..);
        }
    }
    None
}

/// Whether `line` opens or closes a code fence: the fence character and
/// how many of it start the line.
fn fence_of(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let first = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = trimmed.chars().take_while(|&c| c == first).count();
    (run >= 3).then_some((first, run))
}

/// Whether `line` is the `# ` heading `name`, which the generated header
/// already shows.
fn is_title_heading(line: &str, name: &str) -> bool {
    line.strip_prefix("# ").is_some_and(|text| {
        text.trim_end_matches('#').trim().to_lowercase() == name.trim().to_lowercase()
    })
}

/// Tidy a plan body the model sent, without reformatting its Markdown:
///
/// - CRLF becomes LF. A body with no newline whose literal `\n`
///   sequences clearly encode structure (escaped twice by the model; see
///   [`has_escaped_structure`]) gets real newlines.
/// - Trailing whitespace is stripped from every line.
/// - Runs of blank lines outside code fences collapse to one.
/// - A leading `# ` heading equal to `name` (ignoring case) is dropped,
///   since the generated header shows the title.
/// - Leading and trailing blank lines are trimmed; there is no trailing
///   newline.
pub(crate) fn normalize_body(body: &str, name: &str) -> String {
    let mut text = body.replace("\r\n", "\n");
    if !text.contains('\n') && has_escaped_structure(&text) {
        text = unescape_newlines(&text);
    }
    let mut lines = text.lines().map(str::trim_end).skip_while(|l| l.is_empty());
    let mut kept: Vec<&str> = Vec::new();
    match lines.next() {
        Some(first) if is_title_heading(first, name) => {}
        Some(first) => kept.push(first),
        None => return String::new(),
    }
    // The open fence's character and length; it closes on a run at least
    // as long with nothing after it.
    let mut fence: Option<(char, usize)> = None;
    if let Some(&first) = kept.first() {
        fence = fence_of(first);
    }
    for line in lines {
        if kept.is_empty() && line.is_empty() {
            continue;
        }
        match (fence, fence_of(line)) {
            (None, Some(open)) => fence = Some(open),
            (Some((ch, len)), Some((c, run)))
                if c == ch && run >= len && line.trim().chars().all(|x| x == ch) =>
            {
                fence = None;
            }
            (None, None) if line.is_empty() && kept.last().is_some_and(|l| l.is_empty()) => {
                continue;
            }
            _ => {}
        }
        kept.push(line);
    }
    while kept.last().is_some_and(|l| l.is_empty()) {
        kept.pop();
    }
    kept.join("\n")
}

/// Whether a single-line `text` looks like Markdown whose newlines were
/// escaped: some literal `\n` is followed by another one (a blank line)
/// or by a line opener (`#`, `-`, `*`, `>`, a digit or a code fence).
/// A literal `\n` alone, as in "replace `\n` with `\r\n`" or `C:\new`, is
/// prose. An escaped backslash (`\\n`) is never an escape of `n`.
fn has_escaped_structure(text: &str) -> bool {
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        // Consuming the escaped character keeps `\\n` from matching.
        if c != '\\' || chars.next() != Some('n') {
            continue;
        }
        let rest = chars.as_str();
        if rest.starts_with("\\n")
            || rest.starts_with("\\r\\n")
            || rest.starts_with("```")
            || rest.starts_with(|next: char| {
                matches!(next, '#' | '-' | '*' | '>') || next.is_ascii_digit()
            })
        {
            return true;
        }
    }
    false
}

/// `text` with literal `\n` and `\r\n` as newlines; other escapes,
/// including `\\`, are left as they are.
fn unescape_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') if chars.as_str().starts_with("\\n") => {
                chars.nth(1);
                out.push('\n');
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
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
    let body = normalize_body(required_str(args, "plan")?, &name);
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
        let assignee = previous
            .as_ref()
            .and_then(|p| p.meta.todos.iter().find(|t| t.id == todo.id))
            .and_then(|t| t.assignee.clone());
        planned.push(PlanTodo {
            id: todo.id,
            content,
            status,
            assignee,
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
            assignee: None,
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

    fn todo(id: &str, content: &str, status: TodoStatus) -> PlanTodo {
        PlanTodo {
            id: id.into(),
            content: content.into(),
            status,
            assignee: None,
        }
    }

    /// The rendered file after the frontmatter.
    fn readable_part(text: &str) -> &str {
        let rest = text.strip_prefix("---\n").unwrap();
        rest.split_once("\n---\n").unwrap().1
    }

    #[test]
    fn render_shows_title_overview_and_checklist() {
        let mut second = todo("b", "second\nwith details", TodoStatus::InProgress);
        second.assignee = Some("rust-engineer".into());
        let plan = PlanFile {
            meta: PlanMeta {
                name: "Add Parser".into(),
                overview: "Parse things.".into(),
                todos: vec![
                    todo("a", "first", TodoStatus::Completed),
                    second,
                    todo("c", "third", TodoStatus::Pending),
                    todo("d", "fourth", TodoStatus::Cancelled),
                ],
            },
            body: "## Steps\n\nDo it.".into(),
        };
        let text = plan.render();
        assert_eq!(
            readable_part(&text),
            format!(
                "\n# Add Parser\n\nParse things.\n\n## Todos\n\n\
                 - [x] first\n\
                 - [~] second\n  with details (@rust-engineer)\n\
                 - [ ] third\n\
                 - [-] fourth\n\n\
                 {HEADER_END}\n\n## Steps\n\nDo it.\n"
            )
        );
        assert!(text.starts_with("---\nname: Add Parser\n"), "{text}");
    }

    #[test]
    fn render_omits_empty_overview_and_todos_and_ends_in_one_newline() {
        let plan = PlanFile {
            meta: PlanMeta {
                name: "Bare".into(),
                ..PlanMeta::default()
            },
            body: "Body.\n\n".into(),
        };
        assert_eq!(
            readable_part(&plan.render()),
            format!("\n# Bare\n\n{HEADER_END}\n\nBody.\n")
        );
        let empty = PlanFile {
            body: String::new(),
            ..plan
        };
        assert!(empty.render().ends_with(&format!("{HEADER_END}\n")));
    }

    #[test]
    fn rendered_plans_round_trip() {
        let full = PlanFile {
            meta: PlanMeta {
                name: "Full".into(),
                overview: "Over\nview".into(),
                todos: vec![todo("a", "multi\n\nline", TodoStatus::Pending)],
            },
            body: "# Other\n\ntext".into(),
        };
        let bare = PlanFile {
            meta: PlanMeta {
                name: "Bare".into(),
                ..PlanMeta::default()
            },
            body: String::new(),
        };
        let tricky = PlanFile {
            meta: PlanMeta {
                name: "Tricky".into(),
                overview: format!("mentions\n{HEADER_END}\ninside"),
                todos: vec![],
            },
            body: format!("before\n{HEADER_END}\nafter"),
        };
        for plan in [full, bare, tricky] {
            assert_eq!(PlanFile::parse(&plan.render()).unwrap(), plan);
        }
    }

    #[test]
    fn old_files_without_a_header_keep_their_body() {
        let text =
            "---\nname: Old\ntodos:\n- id: a\n  content: first\n---\n\n# Steps\n\nDo it.\n\n";
        let plan = PlanFile::parse(text).unwrap();
        assert_eq!(plan.body, "# Steps\n\nDo it.", "no trailing newline");
        assert_eq!(plan.meta.todos[0].content, "first");
        assert_eq!(PlanFile::parse(&plan.render()).unwrap(), plan, "upgrades");
    }

    #[test]
    fn the_header_marker_is_the_literal_the_terminal_ui_looks_for() {
        // aura-ui-terminal's `PLAN_HEADER_END` repeats this literal (it
        // does not depend on this crate); keep the two in sync.
        assert_eq!(HEADER_END, "<!-- end of generated header: edit below -->");
    }

    #[test]
    fn a_hand_edited_header_is_skipped_at_the_marker() {
        let text = format!("---\nname: P\n---\n\n# Renamed\n\n- [x] stale\n\n{HEADER_END}\n\nBody");
        let plan = PlanFile::parse(&text).unwrap();
        assert_eq!(plan.body, "Body");
        assert!(plan.meta.todos.is_empty(), "todos come from the YAML only");
    }

    #[test]
    fn the_checklist_follows_the_todo_list() {
        let (sandbox, dir) = sandbox();
        let path = new_plan(&sandbox);
        let file = dir.path().join(&path);
        let text = fs::read_to_string(&file).unwrap();
        assert!(text
            .contains("# Add Parser!\n\nParse things\n\n## Todos\n\n- [ ] first\n- [ ] second\n"));

        let mut items = todo_items(&PlanFile::load(&file).unwrap());
        items[0].status = TodoStatus::Completed;
        items[1].status = TodoStatus::InProgress;
        sync_todo_list(&file, &items).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("- [x] first\n- [~] second\n"), "{text}");
        assert!(text.ends_with(&format!("{HEADER_END}\n\n# Steps\n\nDo it.\n")));
    }

    #[test]
    fn normalize_body_tidies_without_reformatting() {
        let cases = [
            ("crlf", "a\r\nb\r\n", "a\nb"),
            (
                "literal newlines",
                "# Steps\\n\\n- one\\n- two",
                "# Steps\n\n- one\n- two",
            ),
            ("real newline keeps literal", "a\\nb\nc", "a\\nb\nc"),
            ("escaped blank line", "Intro\\n\\nMore", "Intro\n\nMore"),
            (
                "escaped crlf structure",
                "# Steps\\r\\n\\r\\n1. one\\r\\n* two",
                "# Steps\n\n1. one\n* two",
            ),
            (
                "escaped quote and fence",
                "Text\\n> quote\\n```\\ncode\\n```",
                "Text\n> quote\n```\ncode\n```",
            ),
            (
                "prose about newlines",
                "Replace `\\n` with `\\r\\n` in `io.rs`.",
                "Replace `\\n` with `\\r\\n` in `io.rs`.",
            ),
            ("windows path", "Write to C:\\new", "Write to C:\\new"),
            (
                "escaped backslashes",
                "a\\\\n\\\\n- b\\\\n# c",
                "a\\\\n\\\\n- b\\\\n# c",
            ),
            (
                "escaped backslash kept when unescaping",
                "C:\\\\new\\n\\n- item",
                "C:\\\\new\n\n- item",
            ),
            ("duplicate title", "\n#  plan NAME \n\nText", "Text"),
            ("other title kept", "# Steps\n\nText", "# Steps\n\nText"),
            (
                "title only first line",
                "Intro\n# Plan Name",
                "Intro\n# Plan Name",
            ),
            ("trailing whitespace", "a  \n\tb\t\n", "a\n\tb"),
            ("blank runs", "\n\na\n\n\n\nb\n \n\n", "a\n\nb"),
            (
                "fences keep blank lines",
                "x\n\n\n```rust\nfn a() {}   \n\n\n\n}\n```\n\n\ny",
                "x\n\n```rust\nfn a() {}\n\n\n\n}\n```\n\ny",
            ),
            (
                "tilde fence with inner backticks",
                "~~~\n```\n\n\n~~~\n\n\nz",
                "~~~\n```\n\n\n~~~\n\nz",
            ),
        ];
        for (label, input, expected) in cases {
            assert_eq!(normalize_body(input, "Plan Name"), expected, "{label}");
        }
    }

    #[test]
    fn create_plan_stores_the_normalized_body() {
        let (sandbox, dir) = sandbox();
        let result = create_plan(
            &sandbox,
            &json!({
                "name": "Tidy",
                "overview": "",
                "plan": "# Tidy\r\n\r\n\r\n\r\nStep one.  \r\n",
                "todos": []
            }),
        )
        .unwrap();
        let path = plan_path_from_output(&output(&result)).unwrap().to_string();
        let text = fs::read_to_string(dir.path().join(&path)).unwrap();
        assert!(!text.contains("## Todos"));
        assert!(text.ends_with(&format!("\n\n# Tidy\n\n{HEADER_END}\n\nStep one.\n")));
        assert_eq!(PlanFile::parse(&text).unwrap().body, "Step one.");
    }
}
