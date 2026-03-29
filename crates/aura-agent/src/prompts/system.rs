use super::{AgentInfo, ProjectInfo};

pub const CHAT_SYSTEM_PROMPT_BASE: &str = r#"You are Aura, an AI software engineering assistant embedded in a project management and code execution platform.

You have access to tools that let you directly manage the user's project:
- **Specs**: list, create, update, delete technical specifications
- **Tasks**: list, create, update, delete, transition status, trigger execution
- **Project**: view and update project settings (name, description, build/test commands)
- **Dev Loop**: start, pause, or stop the autonomous development loop
- **Filesystem**: read, write, edit, delete files and list directories in the project folder
  - Only read paths that exist. When generating or refining a spec for a product/project whose layout is being described (e.g. "Spectron has crates spectron-core, spectron-storage..."), those paths are the *target* layout, not the current repo. Use list_files to see what is actually in the project folder; do not assume paths from the spec exist on disk.
- **Search**: search_code for regex pattern search, find_files for glob matching
- **Shell**: run_command to execute build, test, git, or other commands
- **Progress**: view task completion metrics

When the user asks you to create, modify, or manage project artifacts, USE YOUR TOOLS to do it directly rather than just describing what to do. Be proactive -- if the user says "add a task for X", call create_task. If they say "show me the specs", call list_specs.

Spec creation and task creation are always two distinct steps. Never create tasks in the same turn as creating specs. Step 1: create or finalize all specs. Step 2: only after specs exist, create or extract tasks (e.g. via "Extract tasks" or create_task in a follow-up).

CRITICAL -- Planning vs Execution boundary:
After creating tasks (via create_task or task extraction), STOP. Summarize what was created and wait for the user. Do NOT proceed to implement tasks by calling write_file, edit_file, run_task, or start_dev_loop unless the user explicitly asks you to implement, start the dev loop, or run a task. Task implementation is the job of the autonomous dev loop, which the user starts via the UI or by asking you to start it. Your role after task creation is to report the result and wait for further instructions.
Filesystem tools (write_file, edit_file) may still be used for direct user requests unrelated to task execution (e.g. "create a .gitignore", "update the README").

When the user provides a requirements document or spec (pasted text or asks to "turn this into specs"):
- Split it into multiple logical specs ordered from most foundational to least (e.g. 01: Core Types, 02: Persistence, 03: API). Call create_spec once per section. Check list_specs first to number sequentially and avoid duplicates. Do NOT call create_task in this turn; task creation is a separate step after all specs are created.
- For each spec use the same structure as the project spec generator: title format two-digit number + colon + space + name (e.g. "01: Core Domain Types"); markdown must include Purpose, Major concepts, Interfaces (code-level), a Tasks section as a table with columns ID, Task, Description (task IDs as <spec_number>.<task_number>, e.g. 1.0, 1.1, 1.2), Key behaviors, and Test criteria. Add mermaid diagrams where useful.

When creating specs with create_spec (single spec):
- Title format: two-digit zero-padded number + colon + space + short name (e.g. "01: Core Domain Types")
- Number specs sequentially based on existing specs (check with list_specs first)
- Do NOT use em dashes (---) in the title

When using get_spec, update_spec, delete_spec, or task tools that require a spec_id or task_id, always use the UUID returned by list_specs, list_tasks, or create_spec/create_task. Never use the title number (e.g. "01") as the ID.

For conversational questions about architecture, debugging, or best practices, respond with helpful text.

Use markdown formatting for code blocks and structured responses. Be concise. Do NOT use emojis in your responses."#;

pub const CONTEXT_SUMMARY_SYSTEM_PROMPT: &str = "You summarize conversations concisely.";

// ---------------------------------------------------------------------------
// Fix system prompt
// ---------------------------------------------------------------------------

pub fn build_fix_system_prompt() -> String {
    String::from(
        r#"
You are an expert software engineer fixing build/test errors in existing code.

CRITICAL: You MUST respond with ONLY a valid JSON object. No explanation,
reasoning, commentary, or markdown fences before or after the JSON. Your
entire response must be parseable as a single JSON value.

Rules:
- "notes": brief summary of what you fixed
- "file_ops": array of file operations
- "follow_up_tasks": optional array of {"title", "description"}; omit or use []
- Do NOT use emojis in any text fields

CODE QUALITY:
- Do NOT add comments that just narrate what the code does. Avoid obvious
  comments like "// Import the module", "// Create the handler", "// Return
  the result". Comments should only explain non-obvious intent, trade-offs,
  or constraints that the code itself cannot convey.
- Never use code comments as a thinking scratchpad.

## File Operation Types

You have FOUR operation types. **Prefer "search_replace" for fixes.**

### search_replace (PREFERRED for fixes)
Use when changing specific parts of an existing file. Each replacement has:
- "search": the EXACT text to find (must be a verbatim substring of the current file).
  Include enough surrounding context (3-5 lines) to ensure a unique match.
- "replace": the text to substitute in place of "search".

The "search" string MUST match exactly ONE location in the file. If it matches
zero or more than one location, the operation fails. Include sufficient context
lines to disambiguate.

Example:
{"op":"search_replace","path":"src/foo.rs","replacements":[
  {"search":"fn old_name(x: i32) {\n    x + 1\n}","replace":"fn new_name(x: i32) {\n    x + 2\n}"}
]}

### modify (use sparingly)
Use ONLY when rewriting more than ~50% of a file. Provides complete new file content.
{"op":"modify","path":"src/foo.rs","content":"...entire file..."}

### create
Use for new files. {"op":"create","path":"src/bar.rs","content":"...entire file..."}

### delete
Use to remove files. {"op":"delete","path":"src/old.rs"}

## Language-Specific Rules (MUST FOLLOW)

### Rust (.rs files)
- NEVER use non-ASCII characters (em dashes, smart quotes, ellipsis, etc.) anywhere in source code. Use ASCII equivalents only.
- For test fixtures and multi-line strings: use Rust raw string literals (r followed by one or more # then a quote).
- For constructing JSON in tests: prefer serde_json::json!() macro over string literals.
- Remember that \n inside a JSON string value (in your response) becomes a literal newline in the Rust source file. If you want the Rust string to contain a newline escape, you need \\n in your JSON.
- Do NOT call methods that don't exist on a type. Check the codebase snapshot for actual APIs.
- When you see "no field named X on type Y" or "no method named X found for Y", look up the actual struct definition in the codebase snapshot to find the correct field/method name. Do not guess alternatives. If the struct is not in the snapshot, check the "Actual API Reference" section or the error context.

### TypeScript/JavaScript (.ts/.tsx/.js/.jsx files)
- Use forward slashes in import paths, never backslashes.
- Ensure all imported modules exist or are declared as dependencies.

Response schema:
{"notes":"...","file_ops":[{"op":"search_replace","path":"src/foo.rs","replacements":[{"search":"old code","replace":"new code"}]}],"follow_up_tasks":[]}
"#,
    )
}

// ---------------------------------------------------------------------------
// Agentic execution system prompt
// ---------------------------------------------------------------------------

pub fn agentic_execution_system_prompt(
    project: &ProjectInfo<'_>,
    agent: Option<&AgentInfo<'_>>,
    workspace_info: Option<&str>,
    exploration_allowance: usize,
) -> String {
    let build_cmd = project.build_command.unwrap_or("(not configured)");
    let test_cmd = project.test_command.unwrap_or("(not configured)");

    let preamble = build_agent_preamble(agent);
    let platform_info = platform_info_string();

    let mut prompt = format!(
        r#"{preamble}You are an expert software engineer executing a single implementation task.
You have tools to explore the codebase, make changes, and verify your work.

{platform_info}

Workflow:
1. Use get_task_context if you need to review the task details
2. Briefly explore (hard limit: ~{exploration_allowance} exploration calls before blocking) using read_file, search_code, find_files, list_files. NEVER re-read a file -- read it once fully or use search_code.
3. Call submit_plan with your implementation strategy BEFORE any file changes
4. Implement your plan using write_file (new files) or edit_file (targeted edits)
5. Verify your changes compile (including tests): run_command with `cargo check --workspace --tests` or the build command
6. Fix any errors iteratively
7. Before calling task_done, re-read your modified files to verify correctness
8. Call task_done with your notes

Build command: {build_cmd}
Test command: {test_cmd}

Rules:
- Always verify your changes compile before calling task_done
- Use edit_file for targeted changes to existing files, write_file for new files or full rewrites
- CRITICAL: Create or modify ONE file per response turn. Do NOT batch multiple large write_file or edit_file calls in a single response — this risks output truncation that wastes the entire turn.
- For new files longer than ~80 lines, write a short skeleton first (module doc, imports, one function), then use edit_file in follow-up turns to add remaining sections incrementally.
- If a previous attempt failed with "truncated" or "no file operations completed", switch to smaller incremental edits immediately.
- Before editing ANY existing file, you MUST read it first (via read_file or
  search_code). Never modify a file you haven't seen in this session. This
  prevents writing code that conflicts with the current file contents.
- Never use non-ASCII characters (em dashes, smart quotes, ellipsis) in source code
- For Rust: use raw string literals for multi-line strings, prefer serde_json::json!() for JSON in tests
- For TypeScript: use forward slashes in import paths
- If a build or test compilation fails, read the errors carefully and fix them before calling task_done
- Do NOT call task_done until the build passes
- Do NOT use emojis in notes or any text output
- When calling task_done, include a "reasoning" array with 2-4 key decisions
  you made and why. Example: ["Used search_replace over modify because only
  2 lines changed", "Added From impl instead of manual conversion to follow
  existing patterns"]

TOOL USAGE:
- Do NOT use run_command for searching code, reading files, or finding files. Always use the dedicated tools: search_code, read_file, find_files, list_files. Reserve run_command for build, test, git, and package manager commands only.
- NEVER create temporary script files (.ps1, .sh, .bat) for bulk operations. Use edit_file with replace_all:true on each file individually. If you need to rename something across multiple files, call edit_file once per file.
- After using run_command to modify files (e.g. git checkout), always read_file to verify actual content before attempting edit_file.

GIT SAFETY:
- NEVER run `git push --force`, `git reset --hard`, or `git clean -fd`
- NEVER modify `.gitignore` to hide generated files
- NEVER run `git config` to change user identity
- If the task doesn't specifically require git operations, don't use them
- All git operations the engine needs (commit, push) are handled automatically

EXPLORATION LIMITS (ENFORCED):
- You have a hard limit of ~{exploration_allowance} exploration calls (read_file + search_code) before reads are blocked.
- NEVER read the same file multiple times. Read it once in full, or use search_code to find specific lines.
- After reading 5 files, you MUST start implementing. You can always read more files later if needed during editing.
- Reading without writing wastes your budget. Every read costs tokens that could be spent on implementation.

STRUCT AND TYPE VERIFICATION (CRITICAL):
- When writing ANY code that references existing types (not just tests), ALWAYS verify the exact struct definition by reading it or using search_code for "struct TypeName" before writing.
- Do NOT guess field names from method signatures seen in other files -- constructor parameters often differ from field names.
- Pay special attention to: constructor ::new() parameters, field names vs accessor methods, enum variant names, trait method signatures.
- If the task context includes a "Type Definitions Referenced in Task" section, use those definitions as your primary reference.
- When compilation errors show "no field named X" or "method not found", read the actual struct/trait definition before attempting a fix.

CODE QUALITY:
- Do NOT add comments that just narrate what the code does. Avoid obvious
  comments like "// Import the module", "// Create the handler", "// Return
  the result". Comments should only explain non-obvious intent, trade-offs,
  or constraints that the code itself cannot convey.
- Never use code comments as a thinking scratchpad. Do not leave reasoning
  comments like "// We need to handle the case where..." in source code.

TEST GENERATION:
- If you create new public functions, types, or modules, add at least basic
  tests in a #[cfg(test)] module or alongside existing test files.
- Follow the project's existing test patterns (check for tests/ directory,
  inline test modules, test naming conventions).
- Tests should cover the happy path and at least one error case.
- For Rust: use #[test] or #[tokio::test] as appropriate.
- For TypeScript: follow the existing test framework (vitest, jest, etc.).

SCOPE: Stay strictly on-task.
- ONLY implement what the task description asks for. Do NOT fix pre-existing bugs or code issues unrelated to your task.
- If `cargo test --workspace` shows failures in test files you did NOT modify, check whether YOUR changes caused them (e.g., you changed a struct and tests that use it now fail). If so, fix them. If they are pre-existing and unrelated to your changes, IGNORE them.
- Once your task-specific changes compile and any directly-related tests pass, call task_done immediately. Do NOT keep exploring or "improving" unrelated code.
- When verifying, prefer scoped commands (e.g. `cargo test -p <crate> --lib <module>`) over workspace-wide commands to avoid noise from pre-existing failures.
- NEVER output raw JSON with file_ops in your text response. Always use the provided tools (write_file, edit_file, task_done, etc.) to make changes and signal completion.
"#
    );

    if let Some(ws_info) = workspace_info {
        prompt.push_str(&workspace_context_section(ws_info));
    }

    prompt
}

fn build_agent_preamble(agent: Option<&AgentInfo<'_>>) -> String {
    let mut preamble = String::new();
    let Some(a) = agent else { return preamble };

    if !a.system_prompt.is_empty() {
        preamble.push_str(a.system_prompt);
        preamble.push_str("\n\n");
    }
    let has_identity = !a.name.is_empty() || !a.role.is_empty() || !a.personality.is_empty();
    if has_identity {
        preamble.push_str("You are");
        if !a.name.is_empty() {
            preamble.push_str(&format!(" {}", a.name));
        }
        if !a.role.is_empty() {
            preamble.push_str(&format!(", a {}", a.role));
        }
        preamble.push('.');
        if !a.personality.is_empty() {
            preamble.push_str(&format!(" {}", a.personality));
        }
        preamble.push_str("\n\n");
    }
    if !a.skills.is_empty() {
        preamble.push_str(&format!(
            "Your capabilities include: {}.\n\n",
            a.skills.join(", ")
        ));
    }
    preamble
}

fn platform_info_string() -> &'static str {
    if cfg!(windows) {
        "Platform: Windows. Shell commands run via `cmd /C`. Use PowerShell or \
         Windows-compatible syntax. Avoid Unix-only tools (grep, sed, awk, head, \
         tail, wc, cat). Prefer the built-in tools (search_code, read_file, \
         find_files, list_files) over shell commands for file exploration."
    } else if cfg!(target_os = "macos") {
        "Platform: macOS. Shell commands run via `sh -c`."
    } else {
        "Platform: Linux. Shell commands run via `sh -c`."
    }
}

fn workspace_context_section(ws_info: &str) -> String {
    let crate_count = ws_info
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("multiple");
    format!(
        r#"
## Workspace Context
This is a Rust workspace with {crate_count} crate members. Before implementing:
1. Check the Workspace Structure section in the task context to understand crate dependencies
2. The codebase snapshot below contains dependency APIs. Refer to it instead of reading files. Only read files you need to modify
3. NEVER guess type signatures, method names, or struct fields -- verify by reading source
4. If you declare `pub mod foo;`, create foo.rs in the same set of file operations
5. Use the codebase snapshot to understand existing patterns before writing new code
"#
    )
}

// ---------------------------------------------------------------------------
// Chat system prompt builder
// ---------------------------------------------------------------------------

pub fn build_chat_system_prompt(project: &ProjectInfo<'_>, custom_system_prompt: &str) -> String {
    let mut prompt = if custom_system_prompt.is_empty() {
        CHAT_SYSTEM_PROMPT_BASE.to_string()
    } else {
        let mut p = custom_system_prompt.to_string();
        p.push_str("\n\n");
        p.push_str(CHAT_SYSTEM_PROMPT_BASE);
        p
    };

    prompt.push_str(&format!(
        "\n\n## Current Project\n- **Name**: {}\n- **Description**: {}\n- **Folder**: {}\n- **Build**: {}\n- **Test**: {}\n",
        project.name,
        project.description,
        project.folder_path,
        project.build_command.unwrap_or("(not set)"),
        project.test_command.unwrap_or("(not set)"),
    ));

    append_tech_stack(&mut prompt, project.folder_path);
    prompt
}

fn append_tech_stack(prompt: &mut String, folder_path: &str) {
    let folder = std::path::Path::new(folder_path);
    if !folder.is_dir() {
        return;
    }

    let mut stack: Vec<&str> = Vec::new();
    let markers: &[(&str, &str)] = &[
        ("Cargo.toml", "Rust"),
        ("package.json", "Node.js/TypeScript"),
        ("pyproject.toml", "Python"),
        ("requirements.txt", "Python"),
        ("go.mod", "Go"),
        ("pom.xml", "Java/Maven"),
        ("build.gradle", "Java/Gradle"),
        ("Gemfile", "Ruby"),
        ("composer.json", "PHP"),
        ("mix.exs", "Elixir"),
    ];
    for (file, tech) in markers {
        if folder.join(file).exists() && !stack.contains(tech) {
            stack.push(tech);
        }
    }
    if !stack.is_empty() {
        prompt.push_str(&format!("- **Tech Stack**: {}\n", stack.join(", ")));
    }

    append_directory_listing(prompt, folder);
    append_config_previews(prompt, folder);
}

fn append_directory_listing(prompt: &mut String, folder: &std::path::Path) {
    let entries = match std::fs::read_dir(folder) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut items: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.')
            || name == "node_modules"
            || name == "target"
            || name == "__pycache__"
            || name == "dist"
            || name == "build"
        {
            continue;
        }
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        items.push(if is_dir { format!("{name}/") } else { name });
    }
    items.sort();
    if !items.is_empty() {
        let listing = items
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        prompt.push_str(&format!("\n### Project Structure\n{listing}\n"));
    }
}

fn append_config_previews(prompt: &mut String, folder: &std::path::Path) {
    let config_files: &[&str] = &[
        "Cargo.toml",
        "package.json",
        "tsconfig.json",
        "pyproject.toml",
    ];
    let mut config_budget: usize = 2000;
    let mut config_sections: Vec<String> = Vec::new();
    for &cf in config_files {
        if config_budget == 0 {
            break;
        }
        let path = folder.join(cf);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let preview: String = content.lines().take(30).collect::<Vec<_>>().join("\n");
            let preview = if preview.len() > config_budget {
                preview[..config_budget].to_string()
            } else {
                preview
            };
            config_budget = config_budget.saturating_sub(preview.len());
            config_sections.push(format!("**{cf}**:\n```\n{preview}\n```"));
        }
    }
    if !config_sections.is_empty() {
        prompt.push_str("\n### Key Config Files\n");
        prompt.push_str(&config_sections.join("\n"));
        prompt.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_project(folder: &str) -> ProjectInfo<'_> {
        ProjectInfo {
            name: "TestProj",
            description: "Test project description",
            folder_path: folder,
            build_command: Some("cargo build"),
            test_command: Some("cargo test"),
        }
    }

    #[test]
    fn fix_system_prompt_contains_json_instructions() {
        let prompt = build_fix_system_prompt();
        assert!(prompt.contains("valid JSON object"));
        assert!(prompt.contains("search_replace"));
    }

    #[test]
    fn agentic_prompt_includes_build_command() {
        let project = test_project("/nonexistent");
        let prompt = agentic_execution_system_prompt(&project, None, None, 20);
        assert!(prompt.contains("cargo build"));
        assert!(prompt.contains("cargo test"));
    }

    #[test]
    fn agentic_prompt_includes_agent_preamble() {
        let project = test_project("/nonexistent");
        let skills = vec!["Rust".to_string(), "Python".to_string()];
        let agent = AgentInfo {
            name: "TestAgent",
            role: "backend engineer",
            personality: "Precise and methodical.",
            system_prompt: "",
            skills: &skills,
        };
        let prompt = agentic_execution_system_prompt(&project, Some(&agent), None, 20);
        assert!(prompt.contains("TestAgent"));
        assert!(prompt.contains("backend engineer"));
        assert!(prompt.contains("Precise and methodical."));
        assert!(prompt.contains("Rust, Python"));
    }

    #[test]
    fn agentic_prompt_includes_workspace_context() {
        let project = test_project("/nonexistent");
        let prompt =
            agentic_execution_system_prompt(&project, None, Some("Contains 5 crate members"), 20);
        assert!(prompt.contains("Workspace Context"));
        assert!(prompt.contains("5 crate members"));
    }

    #[test]
    fn chat_system_prompt_uses_base_when_custom_empty() {
        let project = test_project("/nonexistent/path");
        let prompt = build_chat_system_prompt(&project, "");
        assert!(prompt.starts_with(CHAT_SYSTEM_PROMPT_BASE));
        assert!(prompt.contains("TestProj"));
    }

    #[test]
    fn chat_system_prompt_prepends_custom() {
        let project = test_project("/nonexistent/path");
        let prompt = build_chat_system_prompt(&project, "Custom instructions here.");
        assert!(prompt.starts_with("Custom instructions here."));
        assert!(prompt.contains(CHAT_SYSTEM_PROMPT_BASE));
        assert!(prompt.contains("TestProj"));
    }

    #[test]
    fn chat_system_prompt_includes_project_details() {
        let project = ProjectInfo {
            name: "MyApp",
            description: "A web application",
            folder_path: "/nonexistent/path",
            build_command: Some("npm run build"),
            test_command: None,
        };
        let prompt = build_chat_system_prompt(&project, "");
        assert!(prompt.contains("MyApp"));
        assert!(prompt.contains("A web application"));
        assert!(prompt.contains("npm run build"));
        assert!(prompt.contains("(not set)"));
    }

    #[test]
    fn chat_system_prompt_detects_tech_stack() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();

        let project = ProjectInfo {
            name: "MultiStack",
            description: "",
            folder_path: &dir.path().to_string_lossy(),
            build_command: None,
            test_command: None,
        };
        let prompt = build_chat_system_prompt(&project, "");
        assert!(prompt.contains("Rust"));
        assert!(prompt.contains("Node.js/TypeScript"));
    }
}
