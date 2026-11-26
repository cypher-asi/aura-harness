//! Chat system prompt content, AGENTS.md handling and section order.

use super::super::sections::{
    AGENTS_MD_MAX_BYTES, AGENTS_MD_SECTION_TAG_PREFIX, CHAT_SYSTEM_PROMPT_BASE,
};
use super::super::*;
use super::support::test_project;
use crate::descriptors::ProjectInfo;

#[test]
fn chat_system_prompt_uses_base_when_custom_empty() {
    let project = test_project("/nonexistent/path");
    let prompt = build_chat_system_prompt(&project, "", None);
    assert!(prompt.starts_with("<chat_capabilities>\n"));
    assert!(prompt.contains(CHAT_SYSTEM_PROMPT_BASE));
    assert!(prompt.contains("TestProj"));
}

#[test]
fn chat_system_prompt_prepends_custom() {
    let project = test_project("/nonexistent/path");
    let prompt = build_chat_system_prompt(&project, "Custom instructions here.", None);
    assert!(prompt.starts_with("Custom instructions here."));
    assert!(prompt.contains(CHAT_SYSTEM_PROMPT_BASE));
    assert!(prompt.contains("TestProj"));
}

#[test]
fn chat_system_prompt_includes_project_details() {
    let project = ProjectInfo {
        project_id: None,
        name: "MyApp",
        description: "A web application",
        folder_path: "/nonexistent/path",
        build_command: Some("npm run build"),
        test_command: None,
    };
    let prompt = build_chat_system_prompt(&project, "", None);
    assert!(prompt.contains("MyApp"));
    assert!(prompt.contains("A web application"));
    assert!(prompt.contains("npm run build"));
    assert!(
        !prompt.contains("test_command:"),
        "blank test_command must be omitted from <project_context>"
    );
}

#[test]
fn chat_system_prompt_drops_workspace_overview_helpers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();

    let project = ProjectInfo {
        project_id: None,
        name: "MultiStack",
        description: "",
        folder_path: &dir.path().to_string_lossy(),
        build_command: None,
        test_command: None,
    };
    let prompt = build_chat_system_prompt(&project, "", None);
    assert!(
        !prompt.contains("### Project Structure"),
        "Project Structure overview must not appear in the chat prompt: {prompt}"
    );
    assert!(
        !prompt.contains("### Key Config Files"),
        "Key Config Files overview must not appear in the chat prompt: {prompt}"
    );
    assert!(
        !prompt.contains("**Tech Stack**"),
        "Tech Stack overview must not appear in the chat prompt: {prompt}"
    );
}

#[test]
fn chat_system_prompt_includes_agents_md_when_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "Always run cargo check before tests.\nNo emojis.\n",
    )
    .unwrap();

    let folder = dir.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: None,
        name: "WithAgents",
        description: "",
        folder_path: &folder,
        build_command: None,
        test_command: None,
    };
    let prompt = build_chat_system_prompt(&project, "", None);

    assert!(
        prompt.contains(AGENTS_MD_SECTION_TAG_PREFIX),
        "chat prompt is missing the <agents_md path=\"...\"> opening tag"
    );
    assert!(
        prompt.contains("</agents_md>"),
        "chat prompt is missing the closing </agents_md> tag"
    );
    assert!(
        prompt.contains("Always run cargo check before tests."),
        "chat prompt did not include AGENTS.md body"
    );
}

#[test]
fn chat_system_prompt_handles_case_insensitive_variants() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("agents.md"), "Lowercase variant body.").unwrap();

    let folder = dir.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: None,
        name: "LowerAgents",
        description: "",
        folder_path: &folder,
        build_command: None,
        test_command: None,
    };
    let prompt = build_chat_system_prompt(&project, "", None);

    assert!(prompt.contains(AGENTS_MD_SECTION_TAG_PREFIX));
    assert!(prompt.contains("Lowercase variant body."));
    assert!(
        prompt.contains("path=\"AGENTS.md\"") || prompt.contains("path=\"agents.md\""),
        "expected one of the recognised path attribute values in the rendered prompt"
    );
}

#[test]
fn chat_system_prompt_omits_agents_md_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: None,
        name: "NoAgents",
        description: "",
        folder_path: &folder,
        build_command: None,
        test_command: None,
    };
    let prompt = build_chat_system_prompt(&project, "", None);

    assert!(
        !prompt.contains(AGENTS_MD_SECTION_TAG_PREFIX),
        "chat prompt unexpectedly includes <agents_md> when no file is present"
    );
}

#[test]
fn chat_system_prompt_skips_agents_md_when_over_size_cap() {
    let dir = tempfile::tempdir().unwrap();
    let oversize = "x".repeat(AGENTS_MD_MAX_BYTES + 1);
    std::fs::write(dir.path().join("AGENTS.md"), &oversize).unwrap();

    let folder = dir.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: None,
        name: "BigAgents",
        description: "",
        folder_path: &folder,
        build_command: None,
        test_command: None,
    };
    let prompt = build_chat_system_prompt(&project, "", None);

    assert!(
        !prompt.contains(AGENTS_MD_SECTION_TAG_PREFIX),
        "oversize AGENTS.md must be skipped, not truncated"
    );
    assert!(
        !prompt.contains(&oversize),
        "oversize AGENTS.md content must never reach the system prompt"
    );
}

#[test]
fn chat_system_prompt_skips_agents_md_when_folder_missing() {
    let project = ProjectInfo {
        project_id: None,
        name: "Ghost",
        description: "",
        folder_path: "/definitely/does/not/exist/aura/agents/md/test",
        build_command: None,
        test_command: None,
    };
    let prompt = build_chat_system_prompt(&project, "", None);

    assert!(
        !prompt.contains(AGENTS_MD_SECTION_TAG_PREFIX),
        "non-existent folder_path must not surface an <agents_md> section"
    );
}

#[test]
fn chat_system_prompt_includes_codex_derived_sections_in_order() {
    let project = test_project("/nonexistent/path");
    let prompt = build_chat_system_prompt(&project, "", None);

    for tag in [
        "<chat_capabilities>",
        "<project_context>",
        "<editing_etiquette>",
        "<frontend_design>",
        "<output_style>",
    ] {
        assert!(
            prompt.contains(tag),
            "{tag} missing from chat prompt: {prompt}"
        );
    }
    assert!(
        !prompt.contains("<planning_guidance>"),
        "chat path must omit planning_guidance: {prompt}"
    );

    let order = [
        "<chat_capabilities>",
        "<project_context>",
        "<editing_etiquette>",
        "<frontend_design>",
        "<output_style>",
    ];
    let mut last = 0usize;
    for tag in order {
        let idx = prompt.find(tag).expect("tag present above");
        assert!(
            idx >= last,
            "expected {tag} at or after {last}, found {idx}"
        );
        last = idx;
    }
    assert!(
        prompt
            .rfind("<output_style>")
            .expect("output_style present")
            > prompt
                .rfind("</frontend_design>")
                .expect("frontend_design present"),
        "output_style must follow frontend_design: {prompt}"
    );
}
