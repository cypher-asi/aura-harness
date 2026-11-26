use super::*;
use crate::descriptors::FileChangeEntry;

/// Serializes config-mutating bootstrap fence tests (process-wide
/// `aura_config` slot).
static BOOTSTRAP_FENCE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn install_strip_fences(enabled: bool) -> aura_config::ConfigGuard {
    let mut cfg = aura_config::current();
    cfg.agent.prompts.bootstrap_strip_code_fences = enabled;
    aura_config::install_for_test(cfg)
}

/// A first attempt at `task`: no predecessors, work log or enrichment.
fn first_attempt<'a>(
    project: &'a ProjectInfo<'a>,
    spec: &'a SpecInfo<'a>,
    task: &'a TaskInfo<'a>,
    session: &'a SessionInfo<'a>,
) -> BootstrapCtx<'a> {
    BootstrapCtx {
        project,
        spec,
        task,
        session,
        completed_deps: &[],
        work_log_summary: "",
        attempt: 0,
        enrichment_block: None,
    }
}

#[test]
fn basic_context_contains_project_and_task() {
    let project = ProjectInfo {
        project_id: None,
        name: "myproj",
        description: "A test project",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "Spec 1",
        markdown_contents: "spec body",
    };
    let task = TaskInfo {
        title: "Do the thing",
        description: "Implement it",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let ctx = build_agentic_task_context(first_attempt(&project, &spec, &task, &session));
    assert!(ctx.contains("myproj"));
    assert!(ctx.contains("Do the thing"));
    assert!(ctx.contains("Spec 1"));
}

#[test]
fn context_includes_completed_deps() {
    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "s",
        markdown_contents: "",
    };
    let files = vec![FileChangeEntry {
        path: "src/lib.rs".into(),
        op: "modify".into(),
    }];
    let dep = TaskInfo {
        title: "Prior task",
        description: "Did stuff",
        execution_notes: "notes here",
        files_changed: &files,
    };
    let task = TaskInfo {
        title: "Current",
        description: "",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let ctx = build_agentic_task_context(BootstrapCtx {
        completed_deps: &[dep],
        ..first_attempt(&project, &spec, &task, &session)
    });
    assert!(ctx.contains("Prior task"));
    assert!(ctx.contains("src/lib.rs (modify)"));
}

#[test]
fn strip_fenced_code_blocks_removes_python_block() {
    let input =
        "Here is prose.\n\n```python\nimport os\nos.system('rm -rf /')\n```\n\nMore prose.\n";
    let out = strip_fenced_code_blocks(input);
    assert!(out.contains("Here is prose."));
    assert!(out.contains("More prose."));
    assert!(!out.contains("import os"));
    assert!(!out.contains("os.system"));
    assert!(out.contains("[code example:"));
}

#[test]
fn strip_fenced_code_blocks_handles_multiple_fences() {
    let input = "a\n```\none\n```\nb\n```rust\ntwo\n```\nc\n";
    let out = strip_fenced_code_blocks(input);
    assert!(out.contains("a\n"));
    assert!(out.contains("b\n"));
    assert!(out.contains("c\n"));
    assert!(!out.contains("one"));
    assert!(!out.contains("two"));
    assert_eq!(out.matches("[code example:").count(), 2);
}

#[test]
fn strip_fenced_code_blocks_preserves_input_without_fences() {
    let input = "no fences here, just prose with `inline code` and `more`.\n";
    let out = strip_fenced_code_blocks(input);
    assert_eq!(out, input);
}

#[test]
fn task_description_keeps_fences_for_bootstrap_by_default() {
    let _lock = BOOTSTRAP_FENCE_ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _cfg = install_strip_fences(false);

    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "s",
        markdown_contents: "",
    };
    let task = TaskInfo {
        title: "T",
        description: "Do this:\n\n```python\nm.Linear1D(10) & m.Linear1D(5)\n```\n\nDone.",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let ctx = build_agentic_task_context(first_attempt(&project, &spec, &task, &session));
    assert!(ctx.contains("Do this:"));
    assert!(ctx.contains("Done."));
    assert!(ctx.contains("Linear1D"));
    assert!(!ctx.contains("[code example:"));
}

#[test]
fn task_description_strips_fences_when_env_enabled() {
    let _lock = BOOTSTRAP_FENCE_ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _cfg = install_strip_fences(true);

    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "s",
        markdown_contents: "",
    };
    let task = TaskInfo {
        title: "T",
        description: "Do this:\n\n```python\nm.Linear1D(10) & m.Linear1D(5)\n```\n\nDone.",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let ctx = build_agentic_task_context(first_attempt(&project, &spec, &task, &session));
    assert!(ctx.contains("Do this:"));
    assert!(ctx.contains("Done."));
    assert!(!ctx.contains("Linear1D"));
    assert!(ctx.contains("[code example:"));
}

#[test]
fn long_spec_markdown_is_truncated_to_budget() {
    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let big_md = "x".repeat(5_000);
    let spec = SpecInfo {
        title: "Big spec",
        markdown_contents: &big_md,
    };
    let task = TaskInfo {
        title: "Task",
        description: "Do it",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let ctx = build_agentic_task_context(first_attempt(&project, &spec, &task, &session));
    assert!(ctx.contains("# Spec: Big spec"));
    assert!(ctx.contains("spec truncated to fit body cap"));
    assert!(
        ctx.len() < 5_000,
        "expected truncation to keep ctx small, got {}",
        ctx.len()
    );
}

#[test]
fn short_spec_markdown_is_kept_intact() {
    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "Tiny",
        markdown_contents: "tiny body",
    };
    let task = TaskInfo {
        title: "T",
        description: "d",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let ctx = build_agentic_task_context(first_attempt(&project, &spec, &task, &session));
    assert!(ctx.contains("tiny body"));
    assert!(!ctx.contains("spec truncated"));
}

#[test]
fn context_omits_test_warning_for_test_tasks() {
    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "s",
        markdown_contents: "",
    };
    let task = TaskInfo {
        title: "Add integration tests",
        description: "Write tests for the API",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let ctx = build_agentic_task_context(first_attempt(&project, &spec, &task, &session));
    assert!(
        !ctx.contains("This task involves writing tests"),
        "verify-before-writing block must stay out of the task context"
    );
    assert!(
        !ctx.contains("Briefly explore"),
        "explore-first preamble must stay out of the task context"
    );
    assert!(
        ctx.contains("Make the changes this task requires, then call task_done."),
        "round-2 directive must replace the deleted preamble"
    );
}

#[test]
fn build_agentic_task_context_first_attempt_includes_block() {
    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "s",
        markdown_contents: "",
    };
    let task = TaskInfo {
        title: "Implement enqueue",
        description: "Wire Publisher::enqueue in crates/zero-network/src/publisher.rs",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let block = "## Pre-resolved context (from task description)\n\n\
                     Files mentioned in the task that exist in the workspace:\n\
                     - `crates/zero-network/src/publisher.rs`\n\n\
                     Use these as starting points; you do NOT need to re-list \
                     the directory or re-grep for these symbols.\n";
    let ctx = build_agentic_task_context(BootstrapCtx {
        enrichment_block: Some(block),
        ..first_attempt(&project, &spec, &task, &session)
    });
    assert!(ctx.contains("## Pre-resolved context (from task description)"));
    assert!(ctx.contains("crates/zero-network/src/publisher.rs"));
    let task_idx = ctx.find("# Task: Implement enqueue").expect("task");
    let block_idx = ctx.find("## Pre-resolved context").expect("block");
    let directive_idx = ctx
        .find("Make the changes this task requires")
        .expect("dir");
    assert!(
        task_idx < block_idx && block_idx < directive_idx,
        "block must be spliced between task header and directive"
    );
}

#[test]
fn build_agentic_task_context_retry_attempt_skips_block() {
    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "s",
        markdown_contents: "",
    };
    let task = TaskInfo {
        title: "Implement enqueue",
        description: "Wire Publisher::enqueue in crates/zero-network/src/publisher.rs",
        execution_notes: "previous attempt timed out",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let block = "## Pre-resolved context (from task description)\n\n\
                     Files mentioned in the task that exist in the workspace:\n\
                     - `crates/zero-network/src/publisher.rs`\n";
    let ctx = build_agentic_task_context(BootstrapCtx {
        attempt: 1,
        enrichment_block: Some(block),
        ..first_attempt(&project, &spec, &task, &session)
    });
    assert!(!ctx.contains("## Pre-resolved context"));
}

#[test]
fn build_agentic_task_context_empty_block_is_skipped() {
    let project = ProjectInfo {
        project_id: None,
        name: "p",
        description: "",
        folder_path: "/tmp",
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "s",
        markdown_contents: "",
    };
    let task = TaskInfo {
        title: "T",
        description: "d",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let ctx = build_agentic_task_context(BootstrapCtx {
        enrichment_block: Some("   \n"),
        ..first_attempt(&project, &spec, &task, &session)
    });
    assert!(!ctx.contains("Pre-resolved context"));
}
