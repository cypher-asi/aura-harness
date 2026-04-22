//! Tool schema definitions for chat, engine, and multi-project agents.
//!
//! Ported from the app's `aura-tools` crate. Provides lazily-cached
//! [`ToolDefinition`] sets for each agent mode.
//!
//! NOTE: These definitions encode product-level policy (which tools are available
//! in chat vs engine vs multi-project modes). Consider migrating mode-specific
//! tool set composition to the caller (the root `aura` binary, `aura-node`, or a
//! shared session crate) and keeping only individual tool schemas here.

use aura_core::ToolDefinition;

// ============================================================================
// Helpers
// ============================================================================

fn tool(name: &str, description: &str, schema: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        cache_control: None,
        eager_input_streaming: eager_input_streaming_for(name),
    }
}

/// Build a tool definition with property-level descriptions stripped from the
/// JSON schema. Keeps property names, types, enums, required, and nested
/// structure — only removes the verbose "description" field on each property.
fn compact_tool(name: &str, description: &str, schema: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: strip_property_descriptions(schema),
        cache_control: None,
        eager_input_streaming: eager_input_streaming_for(name),
    }
}

/// Tools whose arguments the UI wants to stream live into its preview card
/// (markdown spec body, file contents, diff text). Opting them into
/// Anthropic's fine-grained tool streaming makes `input_json_delta` events
/// arrive as raw partial string bytes while the model writes, instead of
/// being buffered until the full JSON validates at `content_block_stop`.
fn eager_input_streaming_for(name: &str) -> Option<bool> {
    match name {
        "create_spec" | "update_spec" | "write_file" | "edit_file" => Some(true),
        _ => None,
    }
}

fn strip_property_descriptions(mut schema: serde_json::Value) -> serde_json::Value {
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        for (_key, prop_val) in props.iter_mut() {
            if let Some(obj) = prop_val.as_object_mut() {
                obj.remove("description");
            }
        }
    }
    schema
}

// ============================================================================
// Core tools (filesystem, shell, search)
// ============================================================================

pub fn core_tool_definitions() -> Vec<ToolDefinition> {
    let mut tools = filesystem_tools();
    tools.extend(shell_tools());
    tools.extend(search_tools());
    tools
}

fn filesystem_tools() -> Vec<ToolDefinition> {
    let mut tools = file_io_tools();
    tools.extend(file_management_tools());
    tools
}

fn file_io_tools() -> Vec<ToolDefinition> {
    vec![
        tool(
            "read_file",
            "Read the contents of a file relative to the project folder. Optionally read a specific line range (1-indexed) to avoid truncation in large files.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "start_line": { "type": "integer", "description": "First line to read (1-indexed, inclusive). Omit to read from the beginning." },
                    "end_line": { "type": "integer", "description": "Last line to read (1-indexed, inclusive). Omit to read to the end." }
                },
                "required": ["path"]
            }),
        ),
        tool(
            "write_file",
            "Write (create or overwrite) a file relative to the project folder. Best for files under ~150 lines. For larger files, write a skeleton first then use edit_file to fill in sections incrementally.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "content": { "type": "string", "description": "Full file content" }
                },
                "required": ["path", "content"]
            }),
        ),
        tool(
            "edit_file",
            "Make targeted edits to a file by replacing specific text. More efficient than write_file for small changes in large files. The old_text must be an exact match of existing content.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "old_text": { "type": "string", "description": "Exact text to find and replace (must be unique in the file)" },
                    "new_text": { "type": "string", "description": "Replacement text" },
                    "replace_all": { "type": "boolean", "description": "If true, replace all occurrences (default: false, first only)" }
                },
                "required": ["path", "old_text", "new_text"]
            }),
        ),
    ]
}

fn file_management_tools() -> Vec<ToolDefinition> {
    vec![
        tool(
            "delete_file",
            "Delete a file relative to the project folder.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" }
                },
                "required": ["path"]
            }),
        ),
        tool(
            "list_files",
            "List files and directories in a path relative to the project folder.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative directory path (empty or '.' for project root)" }
                },
                "required": []
            }),
        ),
    ]
}

fn shell_tools() -> Vec<ToolDefinition> {
    vec![tool(
        "run_command",
        "Execute a shell command in the project directory. Use ONLY for build, test, git, and package manager commands. Do NOT use for searching code (use search_code), reading files (use read_file), or finding files (use find_files). Commands time out after 60 seconds by default.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to execute" },
                "working_dir": { "type": "string", "description": "Optional relative working directory within the project (default: project root)" },
                "timeout_secs": { "type": "integer", "description": "Timeout in seconds (default: 60, max: 300)" }
            },
            "required": ["command"]
        }),
    )]
}

fn search_tools() -> Vec<ToolDefinition> {
    vec![
        tool(
            "search_code",
            "Search for a regex pattern across files in the project. Returns matching lines with file paths and line numbers. Use context_lines to include surrounding code (e.g. to see a full struct or function body around a match).",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "Optional relative directory or file path to scope the search (default: project root)" },
                    "include": { "type": "string", "description": "Optional glob to filter files, e.g. '*.rs' or '*.ts'" },
                    "max_results": { "type": "integer", "description": "Maximum number of matching lines to return (default: 50)" },
                    "context_lines": { "type": "integer", "description": "Number of lines to include before and after each match (default: 0, max: 10)" }
                },
                "required": ["pattern"]
            }),
        ),
        tool(
            "find_files",
            "Find files by name or glob pattern in the project directory. Returns matching file paths.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match file names, e.g. '*.rs', 'Cargo.toml', 'src/**/*.ts'" },
                    "path": { "type": "string", "description": "Optional relative directory to scope the search (default: project root)" }
                },
                "required": ["pattern"]
            }),
        ),
    ]
}

// ============================================================================
// Chat agent tools
// ============================================================================

pub fn chat_management_tools() -> Vec<ToolDefinition> {
    let mut tools = spec_tool_definitions();
    tools.extend(task_tool_definitions());
    tools.extend(project_tool_definitions());
    tools.extend(dev_loop_tool_definitions());
    tools.extend(orbit_tool_definitions());
    tools.extend(network_tool_definitions());
    tools
}

pub fn orbit_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        compact_tool(
            "orbit_push",
            "Push a branch from the workspace to an orbit git repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"repo":{"type":"string"},"branch":{"type":"string"},"force":{"type":"boolean"}},"required":["org_id","repo","branch"]}),
        ),
        compact_tool(
            "orbit_create_repo",
            "Create a new orbit repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"project_id":{"type":"string"},"owner_id":{"type":"string"},"name":{"type":"string"},"visibility":{"type":"string"}},"required":["org_id","project_id","owner_id","name"]}),
        ),
        compact_tool(
            "orbit_list_repos",
            "List accessible orbit repositories.",
            serde_json::json!({"type":"object","properties":{},"required":[]}),
        ),
        compact_tool(
            "orbit_list_branches",
            "List branches in an orbit repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"repo":{"type":"string"}},"required":["org_id","repo"]}),
        ),
        compact_tool(
            "orbit_create_branch",
            "Create a branch in an orbit repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"repo":{"type":"string"},"name":{"type":"string"},"source":{"type":"string"}},"required":["org_id","repo","name"]}),
        ),
        compact_tool(
            "orbit_list_commits",
            "List recent commits in an orbit repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"repo":{"type":"string"},"ref":{"type":"string"},"limit":{"type":"integer"}},"required":["org_id","repo"]}),
        ),
        compact_tool(
            "orbit_get_diff",
            "Get diff for a commit in an orbit repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"repo":{"type":"string"},"sha":{"type":"string"}},"required":["org_id","repo","sha"]}),
        ),
        compact_tool(
            "orbit_create_pr",
            "Open a pull request in an orbit repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"repo":{"type":"string"},"source_branch":{"type":"string"},"target_branch":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"}},"required":["org_id","repo","source_branch","target_branch","title"]}),
        ),
        compact_tool(
            "orbit_list_prs",
            "List pull requests in an orbit repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"repo":{"type":"string"},"status":{"type":"string"}},"required":["org_id","repo"]}),
        ),
        compact_tool(
            "orbit_merge_pr",
            "Merge a pull request in an orbit repository.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"repo":{"type":"string"},"pr_id":{"type":"string"},"strategy":{"type":"string"}},"required":["org_id","repo","pr_id"]}),
        ),
    ]
}

pub fn network_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        compact_tool(
            "post_to_feed",
            "Post a status update to the activity feed.",
            serde_json::json!({"type":"object","properties":{"profile_id":{"type":"string"},"title":{"type":"string"},"summary":{"type":"string"},"post_type":{"type":"string"},"agent_id":{"type":"string"},"user_id":{"type":"string"},"metadata":{"type":"object"}},"required":["profile_id","title"]}),
        ),
        compact_tool(
            "list_projects",
            "List projects in an organization.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"}},"required":["org_id"]}),
        ),
        compact_tool(
            "check_budget",
            "Check remaining credit budget for a user.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"user_id":{"type":"string"}},"required":["org_id","user_id"]}),
        ),
        compact_tool(
            "record_usage",
            "Record token usage for billing.",
            serde_json::json!({"type":"object","properties":{"org_id":{"type":"string"},"user_id":{"type":"string"},"input_tokens":{"type":"integer"},"output_tokens":{"type":"integer"},"agent_id":{"type":"string"},"model":{"type":"string"}},"required":["org_id","user_id","input_tokens","output_tokens"]}),
        ),
    ]
}

fn spec_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        compact_tool("list_specs", "List all specs in the current project.", serde_json::json!({"type":"object","properties":{},"required":[]})),
        compact_tool("get_spec", "Get a single spec by its UUID spec_id (from list_specs or create_spec output, NOT the title number).", serde_json::json!({"type":"object","properties":{"spec_id":{"type":"string","description":"The spec ID"}},"required":["spec_id"]})),
        compact_tool("create_spec", "Create a new spec. When creating from a requirements document, create one spec per logical phase (multiple calls); title format '01: Name', '02: Name'; markdown: Purpose, Interfaces, Tasks table (1.0/1.1), Test criteria. Do not create tasks in the same step — task creation is always a separate step after all specs exist.", serde_json::json!({"type":"object","properties":{"title":{"type":"string"},"markdown_contents":{"type":"string"}},"required":["title","markdown_contents"]})),
        compact_tool("update_spec", "Update an existing spec's title or contents. Use the UUID spec_id from list_specs.", serde_json::json!({"type":"object","properties":{"spec_id":{"type":"string"},"title":{"type":"string"},"markdown_contents":{"type":"string"}},"required":["spec_id"]})),
        compact_tool("delete_spec", "Delete a spec and its tasks from the project. Use the UUID spec_id from list_specs.", serde_json::json!({"type":"object","properties":{"spec_id":{"type":"string"}},"required":["spec_id"]})),
    ]
}

fn task_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        compact_tool("list_tasks", "List all tasks in the project, optionally filtered by UUID spec_id from list_specs.", serde_json::json!({"type":"object","properties":{"spec_id":{"type":"string"}},"required":[]})),
        compact_tool("create_task", "Create a new task under a spec. Use the UUID spec_id from list_specs. Only use after specs exist; never create tasks in the same turn as creating specs — spec creation and task creation are two distinct steps.", serde_json::json!({"type":"object","properties":{"spec_id":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"},"dependency_ids":{"type":"array","items":{"type":"string"},"description":"UUIDs of tasks this task depends on (from list_tasks)"}},"required":["spec_id","title","description"]})),
        compact_tool("update_task", "Update a task's title, description, or status.", serde_json::json!({"type":"object","properties":{"task_id":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"},"status":{"type":"string","enum":["pending","ready","in_progress","blocked","done","failed"]}},"required":["task_id"]})),
        compact_tool("delete_task", "Delete a task from the project. Requires UUID task_id and parent UUID spec_id from list_tasks.", serde_json::json!({"type":"object","properties":{"task_id":{"type":"string"},"spec_id":{"type":"string"}},"required":["task_id","spec_id"]})),
        compact_tool("transition_task", "Transition a task to a new status (e.g. pending -> ready, ready -> done).", serde_json::json!({"type":"object","properties":{"task_id":{"type":"string"},"status":{"type":"string","enum":["pending","ready","in_progress","blocked","done","failed"]}},"required":["task_id","status"]})),
        compact_tool("run_task", "Trigger execution of a single task by the dev-loop engine.", serde_json::json!({"type":"object","properties":{"task_id":{"type":"string"}},"required":["task_id"]})),
    ]
}

fn project_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        compact_tool("get_project", "Get the current project's details (name, folder, status, etc.).", serde_json::json!({"type":"object","properties":{},"required":[]})),
        compact_tool("update_project", "Update the current project's name, description, build_command, or test_command. Commands must be valid shell commands with no extra text.", serde_json::json!({"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"build_command":{"type":"string"},"test_command":{"type":"string"}},"required":[]})),
    ]
}

fn dev_loop_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        compact_tool("start_dev_loop", "Start the autonomous dev loop for the project. It will pick up ready tasks and execute them.", serde_json::json!({"type":"object","properties":{},"required":[]})),
        compact_tool("pause_dev_loop", "Pause the currently running dev loop.", serde_json::json!({"type":"object","properties":{},"required":[]})),
        compact_tool("stop_dev_loop", "Stop the currently running dev loop.", serde_json::json!({"type":"object","properties":{},"required":[]})),
    ]
}

// ============================================================================
// Engine tools
// ============================================================================

pub fn engine_specific_tools() -> Vec<ToolDefinition> {
    vec![
        tool(
            "task_done",
            "Signal that the current task is complete. Call this when you have finished all changes and verified they compile. Provide notes summarizing what you did, optionally follow-up task suggestions, and a reasoning array with key decisions.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "notes": { "type": "string", "description": "Summary of what was done" },
                    "follow_ups": {
                        "type": "array",
                        "description": "Optional follow-up task suggestions",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "description": { "type": "string" }
                            },
                            "required": ["title", "description"]
                        }
                    },
                    "reasoning": {
                        "type": "array",
                        "description": "Key decisions and their rationale (optional but encouraged)",
                        "items": { "type": "string" }
                    }
                },
                "required": ["notes"]
            }),
        ),
        tool(
            "get_task_context",
            "Retrieve the full context for the current task including the spec, task description, and any prior execution notes.",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        ),
        tool(
            "submit_plan",
            "Submit your implementation plan before making any file changes. \
             You MUST call this after exploration and before any write_file/edit_file \
             calls. The plan is validated and becomes your reference during implementation.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "approach": {
                        "type": "string",
                        "description": "Your implementation strategy (2-4 sentences)"
                    },
                    "files_to_modify": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Existing files you will edit"
                    },
                    "files_to_create": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New files you will create"
                    },
                    "key_decisions": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Key design decisions and why"
                    }
                },
                "required": ["approach", "files_to_modify", "files_to_create"]
            }),
        ),
    ]
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_property_descriptions_removes_descriptions() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The name" },
                "age": { "type": "integer", "description": "The age" }
            }
        });
        let stripped = strip_property_descriptions(schema);
        let props = stripped.get("properties").unwrap();
        assert!(props.get("name").unwrap().get("description").is_none());
        assert!(props.get("age").unwrap().get("description").is_none());
    }

    #[test]
    fn streaming_tools_opt_into_eager_input_streaming() {
        let fs_tools = file_io_tools();
        let spec_tools = spec_tool_definitions();

        for name in ["write_file", "edit_file"] {
            let t = fs_tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("missing {name} in file_io_tools"));
            assert_eq!(
                t.eager_input_streaming,
                Some(true),
                "{name} must opt into fine-grained tool streaming"
            );
        }

        for name in ["create_spec", "update_spec"] {
            let t = spec_tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("missing {name} in spec_tool_definitions"));
            assert_eq!(
                t.eager_input_streaming,
                Some(true),
                "{name} must opt into fine-grained tool streaming"
            );
        }

        let read = fs_tools.iter().find(|t| t.name == "read_file").unwrap();
        assert_eq!(
            read.eager_input_streaming, None,
            "read_file has nothing large to stream; should not set the flag"
        );
    }

    #[test]
    fn strip_property_descriptions_preserves_types() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path" },
                "count": { "type": "integer", "description": "Count" }
            }
        });
        let stripped = strip_property_descriptions(schema);
        let props = stripped.get("properties").unwrap();
        assert_eq!(props.get("path").unwrap().get("type").unwrap(), "string");
        assert_eq!(props.get("count").unwrap().get("type").unwrap(), "integer");
    }
}
