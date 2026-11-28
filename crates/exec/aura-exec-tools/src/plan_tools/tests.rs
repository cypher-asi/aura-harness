use super::body::normalize_body;
use super::create::slug;
use super::*;
use crate::error::ToolError;
use crate::sandbox::Sandbox;
use aura_core_types::ToolResult;
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
            ..PlanMeta::default()
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
            ..PlanMeta::default()
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
            ..PlanMeta::default()
        },
        body: format!("before\n{HEADER_END}\nafter"),
    };
    for plan in [full, bare, tricky] {
        assert_eq!(PlanFile::parse(&plan.render()).unwrap(), plan);
    }
}

#[test]
fn old_files_without_a_header_keep_their_body() {
    let text = "---\nname: Old\ntodos:\n- id: a\n  content: first\n---\n\n# Steps\n\nDo it.\n\n";
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
    assert!(
        text.contains("# Add Parser!\n\nParse things\n\n## Todos\n\n- [ ] first\n- [ ] second\n")
    );

    let mut items = todo_items(&PlanFile::load(&file).unwrap());
    items[0].status = TodoStatus::Completed;
    items[1].status = TodoStatus::InProgress;
    sync_todo_list(&file, &items).unwrap();
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains("- [x] first\n- [~] second\n"), "{text}");
    assert!(text.ends_with(&format!("{HEADER_END}\n\n# Steps\n\nDo it.\n")));
}

#[test]
fn normalize_body_unescapes_only_structural_newlines() {
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
    ];
    for (label, input, expected) in cases {
        assert_eq!(normalize_body(input, "Plan Name"), expected, "{label}");
    }
}

#[test]
fn normalize_body_tidies_titles_blank_lines_and_fences() {
    let cases = [
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
