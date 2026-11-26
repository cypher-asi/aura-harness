use super::support::*;
use super::*;

#[test]
fn task_tool_lists_permanent_agents() {
    let fixture = fixture("unused");
    let tool = task_tool_definition(&lock(&fixture.hub.definitions)).unwrap();

    assert_eq!(tool.name, TASK_TOOL_NAME);
    assert_eq!(
        tool.input_schema["properties"]["subagent_type"]["enum"],
        serde_json::json!([
            "engineer",
            "explorer",
            "frontend-engineer",
            "implement",
            "product-manager",
            "reviewer",
            "rust-engineer",
            "tests",
            "ui-designer",
            "web-designer"
        ])
    );
    assert!(tool.description.contains("- reviewer: "));
    assert!(tool
        .description
        .contains("- ui-designer: Product UI designer"));
}

#[test]
fn no_task_tool_without_permanent_agents() {
    let data = TempDir::new().unwrap();
    let mut definitions = AgentDefinitions::load_or_seed(data.path()).unwrap();
    let names: Vec<String> = definitions.all().iter().map(|d| d.name.clone()).collect();
    for name in names {
        definitions.delete(&name).unwrap();
    }

    assert!(task_tool_definition(&definitions).is_none());
}

#[test]
fn turn_applies_definition_rules_and_tools() {
    let fixture = fixture("unused");
    let workspace = fixture.workspace.path();

    let (_, plain_tools) = fixture.hub.turn(None, workspace);
    assert!(plain_tools.iter().any(|t| t.name == TASK_TOOL_NAME));

    let (config, tools) = fixture.hub.turn(Some("explorer"), workspace);
    assert!(config
        .system_prompt
        .contains("<agent_definition name=\"explorer\">"));
    assert!(tools.iter().all(|t| t.name != TASK_TOOL_NAME));
    assert!(tools.iter().any(|t| t.name == "read_file"));
    assert!(tools.iter().all(|t| t.name != "write_file"));

    let (config, tools) = fixture.hub.turn(Some("ui-designer"), workspace);
    assert!(config.system_prompt.contains("--accent-ink: #1a1a1a;"));
    assert!(tools.iter().any(|t| t.name == "write_file"));
    assert!(tools.iter().all(|t| t.name != TASK_TOOL_NAME));
}

#[test]
fn turn_puts_project_instructions_in_the_stable_prompt_prefix() {
    let fixture = fixture("unused");
    let workspace = fixture.workspace.path();
    let base = lock(&fixture.hub.base).config.system_prompt.clone();
    let (config, _) = fixture.hub.turn(Some("explorer"), workspace);
    assert!(!config.system_prompt.contains("<project_instructions>"));

    std::fs::write(
        workspace.join("AGENTS.md"),
        "Run `make check` before committing.",
    )
    .unwrap();
    let (config, _) = fixture.hub.turn(Some("explorer"), workspace);
    let prompt = config.system_prompt;
    let instructions = prompt.find("<project_instructions>").unwrap();
    let definition = prompt.find("<agent_definition").unwrap();
    assert!(prompt.starts_with(&format!("{base}\n\n<project_instructions>")));
    assert!(instructions < definition);
    assert!(prompt.contains("Run `make check` before committing."));

    let (again, _) = fixture.hub.turn(Some("explorer"), workspace);
    assert_eq!(
        again.system_prompt, prompt,
        "unchanged files keep the prompt cacheable"
    );
}


#[test]
fn task_tool_offers_detached_spawns() {
    let fixture = fixture("unused");
    let tool = task_tool_definition(&lock(&fixture.hub.definitions)).unwrap();
    assert_eq!(
        tool.input_schema["properties"]["spawn_mode"]["enum"],
        serde_json::json!(["wait", "detached"])
    );
}

