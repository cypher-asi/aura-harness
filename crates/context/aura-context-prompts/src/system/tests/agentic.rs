//! Dev-loop (agentic execution) prompt content and section order.

use super::super::sections::AGENTS_MD_SECTION_TAG_PREFIX;
use super::super::*;
use super::support::test_project;
use crate::descriptors::{AgentIdentity, AgentInfo, ProjectInfo};

#[test]
fn agentic_prompt_includes_build_command() {
    let project = test_project("/nonexistent");
    let prompt = agentic_execution_system_prompt(&project, None, None);
    assert!(prompt.contains("cargo build"));
    assert!(prompt.contains("cargo test"));
}

#[test]
fn agentic_prompt_includes_definition_of_done_hard_gate() {
    let project = test_project("/nonexistent");
    let prompt = agentic_execution_system_prompt(&project, None, None);
    assert!(
        prompt.contains("hard gate"),
        "task_done hard-gate language missing: {prompt}"
    );
    assert!(
        prompt.contains("When calling task_done:"),
        "task_done deferral instruction missing: {prompt}"
    );
    assert!(
        prompt.contains("cargo test"),
        "test command reference missing from hard-gate line: {prompt}"
    );
}

#[test]
fn agentic_prompt_no_longer_tells_agent_to_ignore_pre_existing_failures() {
    let project = test_project("/nonexistent");
    let prompt = agentic_execution_system_prompt(&project, None, None);
    assert!(
        !prompt.contains("IGNORE them"),
        "system prompt still tells agent to IGNORE pre-existing failures"
    );
    assert!(
        !prompt.contains("If they are pre-existing and unrelated to your changes"),
        "system prompt still contains the old pre-existing-failure escape hatch"
    );
}

#[test]
fn agentic_prompt_no_longer_advertises_exploration_budget() {
    let project = test_project("/nonexistent");
    let prompt = agentic_execution_system_prompt(&project, None, None);

    assert!(
        !prompt.contains("EXPLORATION BUDGET"),
        "EXPLORATION BUDGET section was supposed to be removed: {prompt}"
    );
    assert!(
        !prompt.contains("read-only tool calls"),
        "per-call exploration cap prose was supposed to be removed: {prompt}"
    );
    assert!(
        !prompt.contains("Each file can be read at most"),
        "per-file read-cap prose was supposed to be removed: {prompt}"
    );
    assert!(
        !prompt.contains("the only legal moves are"),
        "post-budget legal-move prose was supposed to be removed: {prompt}"
    );
}

#[test]
fn agentic_prompt_leads_with_action_oriented_move_set() {
    let project = test_project("/nonexistent");
    let prompt = agentic_execution_system_prompt(&project, None, None);

    assert!(
        prompt.contains(
            "Edit code with write_file / edit_file / delete_file. Call task_done for structured notes"
        ),
        "action-oriented lead line missing: {prompt}"
    );
    assert!(
        !prompt.contains("Workflow:"),
        "old numbered Workflow header should be gone: {prompt}"
    );
    assert!(
        !prompt.contains("1. Explore"),
        "old step-1 Explore prose should be gone: {prompt}"
    );
    assert!(
        prompt.contains("submit_plan"),
        "dev-loop prompt must advertise submit_plan as an optional commitment device: {prompt}"
    );
}

#[test]
fn agentic_prompt_promotes_no_changes_needed_in_rules() {
    let project = test_project("/nonexistent");
    let prompt = agentic_execution_system_prompt(&project, None, None);

    assert!(
        prompt.contains("no_changes_needed: true"),
        "no_changes_needed branch missing from invariants: {prompt}"
    );
    assert!(
        prompt.contains("If no changes are needed"),
        "no_changes_needed conditional phrasing missing: {prompt}"
    );
}

#[test]
fn agentic_prompt_emits_tool_discipline_envelope_for_live_runtime_gates() {
    let project = test_project("/nonexistent");
    let prompt = agentic_execution_system_prompt(&project, None, None);

    assert!(
        prompt.contains("<tool_discipline>"),
        "tool_discipline section must be live again after the backfill: {prompt}"
    );
    assert!(
        prompt.contains("</tool_discipline>"),
        "tool_discipline envelope must close: {prompt}"
    );
    assert!(
        prompt.contains("32000 bytes"),
        "tool_discipline must surface the write_file chunk cap: {prompt}"
    );
    assert!(
        prompt.contains("_redacted"),
        "tool_discipline must surface the compaction-redaction guard: {prompt}"
    );
    // Stripped-rule regression guards: the rules the harness no longer
    // enforces must not reappear via the new body.
    assert!(
        !prompt.contains("Tool-call discipline:"),
        "old plaintext header must not return: {prompt}"
    );
    assert!(
        !prompt.contains("cargo check"),
        "cargo subcommand denial is no longer enforced and must not return: {prompt}"
    );
}

#[test]
fn agentic_prompt_uses_test_command_env_override_when_set() {
    let project = test_project("/nonexistent");
    let prompt = agentic_execution_system_prompt(&project, None, Some("pytest -q -k smoke"));
    assert!(
        prompt.contains("pytest -q -k smoke"),
        "override must surface in the prompt"
    );
}

#[test]
fn agentic_prompt_includes_agents_md_when_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "Use raw string literals for multi-line Rust strings.",
    )
    .unwrap();

    let folder = dir.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: None,
        name: "AgenticAgents",
        description: "",
        folder_path: &folder,
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    };
    let prompt = agentic_execution_system_prompt(&project, None, None);

    assert!(prompt.contains(AGENTS_MD_SECTION_TAG_PREFIX));
    assert!(prompt.contains("Use raw string literals for multi-line Rust strings."));
}

#[test]
fn agentic_prompt_omits_agents_md_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: None,
        name: "AgenticNoAgents",
        description: "",
        folder_path: &folder,
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    };
    let prompt = agentic_execution_system_prompt(&project, None, None);

    assert!(
        !prompt.contains(AGENTS_MD_SECTION_TAG_PREFIX),
        "agentic prompt unexpectedly includes <agents_md> when no file is present"
    );
}

#[test]
fn agentic_prompt_skips_agents_md_when_folder_missing() {
    let project = test_project("/definitely/does/not/exist/aura/agentic/test");
    let prompt = agentic_execution_system_prompt(&project, None, None);

    assert!(
        !prompt.contains(AGENTS_MD_SECTION_TAG_PREFIX),
        "non-existent folder_path must not surface <agents_md> in the agentic prompt"
    );
}

#[test]
fn dev_loop_prompt_with_identity_emits_every_section_in_order() {
    let project = test_project("/nonexistent");
    let skills = vec!["Rust".to_string(), "TypeScript".to_string()];
    let agent = AgentInfo {
        identity: Some(AgentIdentity {
            name: "Atlas",
            role: "Engineer",
            personality: "Precise and methodical.",
        }),
        skills: &skills,
        system_prompt: Some("Use TDD on every change."),
    };
    let prompt = agentic_execution_system_prompt(&project, Some(&agent), None);

    let order = [
        "<agent_identity>",
        "<agent_skills>",
        "<agent_system_prompt>",
        "<project_context>",
        "<dev_loop_workflow>",
        "<tool_discipline>",
        "<editing_etiquette>",
        "<planning_guidance>",
        "<frontend_design>",
        "<output_style>",
    ];
    let mut last = 0usize;
    for tag in order {
        let idx = prompt
            .find(tag)
            .unwrap_or_else(|| panic!("{tag} missing from prompt: {prompt}"));
        assert!(
            idx >= last,
            "expected {tag} to appear at or after offset {last}, but found it at {idx}; prompt:\n{prompt}"
        );
        last = idx;
    }
}
