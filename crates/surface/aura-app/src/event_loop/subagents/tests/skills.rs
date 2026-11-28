//! A permanent agent's skills on a local turn: listed as an index, and
//! loaded in full with `load_skill`.

use super::*;
use aura_exec_tools::skills::{LoadSkillTool, SkillSource, LOAD_SKILL_TOOL_NAME};
use aura_exec_tools::{Sandbox, Tool, ToolConfig, ToolContext};

/// The body of the `deploy` skill the fixtures write.
const DEPLOY_BODY: &str = "Tag the release, build it, then publish the notes.";

/// `fixture` with a `deploy` skill in its workspace and a `builder`
/// permanent agent listing it, with a `tools:` allow-list that doesn't
/// name `load_skill`.
fn with_a_skilled_definition() -> Fixture {
    let fixture = fixture("unused");
    let skill = fixture.workspace.path().join("skills/deploy");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        format!("---\nname: deploy\ndescription: Ship a release.\n---\n{DEPLOY_BODY}\n"),
    )
    .unwrap();
    std::fs::write(
        fixture._data.path().join("agent_defs/builder.md"),
        "---\nname: builder\ndescription: Builds.\ntools: [read_file]\nskills: [deploy]\n---\nBe careful.\n",
    )
    .unwrap();
    assert!(fixture.hub.reload_definitions().is_empty());
    fixture
}

#[test]
fn a_local_turn_lists_its_skills_and_offers_load_skill() {
    let fixture = with_a_skilled_definition();
    let workspace = fixture.workspace.path().to_path_buf();
    let (config, tools) = fixture.hub.turn(Some("builder"), &workspace);
    assert!(config.system_prompt.contains("name=\"deploy\""));
    assert!(config.system_prompt.contains("Ship a release."));
    assert!(
        !config.system_prompt.contains(DEPLOY_BODY),
        "no body on every turn"
    );
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&LOAD_SKILL_TOOL_NAME), "{names:?}");
    assert!(names.contains(&"read_file"));

    let (plain, plain_tools) = fixture.hub.turn(None, &workspace);
    assert!(!plain.system_prompt.contains("agent_skills"));
    assert!(!plain_tools.iter().any(|t| t.name == LOAD_SKILL_TOOL_NAME));
}

#[test]
fn a_cloud_turn_still_carries_its_skills_in_full() {
    let fixture = with_a_skilled_definition();
    let agent = lock(&fixture.hub.registry)
        .create_instance("builder", None)
        .unwrap()
        .agent_id();
    let ask = fixture.hub.cloud_turn(agent);
    let prompt = ask.system_prompt.unwrap_or_default();
    assert!(prompt.contains(DEPLOY_BODY), "the machine can't load it");
}

#[tokio::test]
async fn load_skill_returns_the_callers_skill_in_full() {
    let fixture = with_a_skilled_definition();
    let agent = lock(&fixture.hub.registry)
        .create_instance("builder", None)
        .unwrap()
        .agent_id();
    let link = crate::event_loop::HubLink::default();
    link.set(&fixture.hub);
    let source = Arc::new(DefinitionSkills::new(link));
    let tool = LoadSkillTool::new(source.clone());
    let mut ctx = ToolContext::new(
        Sandbox::new(fixture.workspace.path()).unwrap(),
        ToolConfig::default(),
    );
    ctx.caller_agent_id = Some(agent);
    let result = tool
        .execute(&ctx, serde_json::json!({"name": "deploy"}))
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&result.stdout).contains(DEPLOY_BODY));

    let unknown = source.load(Some(agent), "launch").unwrap_err();
    assert!(unknown.contains("your skills: deploy"), "{unknown}");
    let own = lock(&fixture.hub.registry).active().agent_id();
    assert!(
        source.load(Some(own), "deploy").is_err(),
        "the user's own agent has none"
    );
}

#[test]
fn the_tool_and_the_index_agree_on_its_name() {
    assert_eq!(
        LOAD_SKILL_TOOL_NAME,
        aura_context_skills::on_demand::LOAD_SKILL_TOOL_NAME
    );
}
