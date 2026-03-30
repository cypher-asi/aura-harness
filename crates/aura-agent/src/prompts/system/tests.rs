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
