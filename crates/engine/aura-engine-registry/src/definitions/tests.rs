use super::*;

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(name, "", serde_json::json!({"type": "object"}))
}

#[test]
fn parses_frontmatter_and_rules() {
    let content = "---\nname: reviewer\ndescription: Reviews diffs.\nmodel: claude-x\n\
                       tools: [read_file]\nskills: [deploy]\n---\nBe strict.\n\nCite files.\n";
    let def = AgentDefinition::parse(content, Path::new("reviewer.md")).unwrap();
    assert_eq!(def.name, "reviewer");
    assert_eq!(def.description, "Reviews diffs.");
    assert_eq!(def.model.as_deref(), Some("claude-x"));
    assert_eq!(def.tools, Some(vec!["read_file".to_string()]));
    assert_eq!(def.skills, vec!["deploy".to_string()]);
    assert_eq!(def.rules, "Be strict.\n\nCite files.");
}

#[test]
fn rejects_missing_frontmatter_and_bad_names() {
    assert!(AgentDefinition::parse("no frontmatter", Path::new("x.md")).is_err());
    assert!(AgentDefinition::parse("---\nname: x\n", Path::new("x.md")).is_err());
    assert!(AgentDefinition::parse("---\nname: Bad Name\n---\n", Path::new("x.md")).is_err());
    assert!(validate_name("ok-name_2").is_ok());
    assert!(validate_name("-leading").is_err());
    assert!(validate_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
}

const SEEDED_NAMES: [&str; 11] = [
    "coordinator",
    "engineer",
    "explorer",
    "frontend-engineer",
    "implement",
    "product-manager",
    "reviewer",
    "rust-engineer",
    "tests",
    "ui-designer",
    "web-designer",
];

fn names(defs: &AgentDefinitions) -> Vec<&str> {
    defs.all().iter().map(|d| d.name.as_str()).collect()
}

#[test]
fn every_seed_parses_under_its_own_name() {
    for (name, content) in SEED_DEFINITIONS {
        let def = AgentDefinition::parse(content, Path::new("seed.md"))
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(def.name, *name);
        assert!(!def.description.is_empty(), "{name} has no description");
        assert!(!def.rules.is_empty(), "{name} has no rules");
    }
}

#[test]
fn seeds_starter_definitions_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
    assert_eq!(names(&defs), SEEDED_NAMES);

    defs.delete("explorer").unwrap();
    defs.delete("ui-designer").unwrap();
    let reloaded = AgentDefinitions::load_or_seed(dir.path()).unwrap();
    assert_eq!(reloaded.all().len(), SEEDED_NAMES.len() - 2);
    assert!(reloaded.get("explorer").is_none());
    assert!(reloaded.get("ui-designer").is_none());
    assert!(reloaded.get("REVIEWER").is_some());
}

#[test]
fn seeds_new_defaults_into_a_directory_from_before_the_seed_list() {
    let dir = tempfile::tempdir().unwrap();
    let defs_dir = dir.path().join(DEFINITIONS_DIR);
    fs::create_dir_all(&defs_dir).unwrap();
    fs::write(
        defs_dir.join("reviewer.md"),
        "---\nname: reviewer\n---\nMine.\n",
    )
    .unwrap();
    fs::write(
        defs_dir.join("web.md"),
        "---\nname: web-designer\n---\nMine.\n",
    )
    .unwrap();

    let defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
    assert!(
        defs.get("explorer").is_none(),
        "a deleted legacy seed came back"
    );
    assert_eq!(defs.get("reviewer").unwrap().rules, "Mine.");
    assert_eq!(defs.get("web-designer").unwrap().rules, "Mine.");
    assert!(!defs_dir.join("web-designer.md").exists());
    for name in [
        "engineer",
        "rust-engineer",
        "frontend-engineer",
        "ui-designer",
        "product-manager",
        "implement",
        "tests",
    ] {
        assert!(defs.get(name).is_some(), "missing {name}");
    }
}

#[test]
fn create_writes_a_loadable_skeleton_with_a_free_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
    let first = defs.create(None).unwrap();
    let second = defs.create(None).unwrap();
    assert!(first.ends_with("new-agent.md"));
    assert!(second.ends_with("new-agent-2.md"));
    assert!(defs.get("new-agent-2").is_some());

    defs.create(Some("Planner")).unwrap();
    assert!(defs.get("planner").is_some());
    assert!(defs.create(Some("planner")).is_err());
    assert!(defs.create(Some("not valid")).is_err());
}

#[test]
fn reload_reports_broken_and_duplicate_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut defs = AgentDefinitions::load_or_seed(dir.path()).unwrap();
    let defs_dir = dir.path().join(DEFINITIONS_DIR);
    fs::write(defs_dir.join("broken.md"), "not a definition").unwrap();
    fs::write(defs_dir.join("zz-copy.md"), "---\nname: reviewer\n---\n").unwrap();

    let problems = defs.reload();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert_eq!(defs.all().len(), SEEDED_NAMES.len());
}

#[test]
fn filter_tools_honors_the_allow_list_and_hides_task_by_default() {
    let tools = [tool("read_file"), tool("write_file"), tool("task")];
    let mut def = AgentDefinition::parse("---\nname: a\n---\n", Path::new("a.md")).unwrap();
    let names = |def: &AgentDefinition| -> Vec<String> {
        def.filter_tools(&tools)
            .into_iter()
            .map(|t| t.name)
            .collect()
    };
    assert_eq!(names(&def), ["read_file", "write_file"]);

    def.tools = Some(vec!["read_file".into(), "task".into()]);
    assert_eq!(names(&def), ["read_file", "task"]);
}

fn seed_content(name: &str) -> &'static str {
    SEED_DEFINITIONS
        .iter()
        .find(|(seed, _)| *seed == name)
        .map(|(_, content)| *content)
        .unwrap()
}

#[test]
fn coordinator_seed_lists_only_coordination_tools() {
    let content = seed_content(COORDINATOR_DEFINITION);
    let def = AgentDefinition::parse(content, Path::new("coordinator.md")).unwrap();
    assert!(def.is_coordinator());
    let listed = def.tools.unwrap();
    assert!(listed.iter().any(|tool| tool == ASSIGN_TOOL_NAME));
    assert!(
        listed
            .iter()
            .all(|tool| COORDINATOR_TOOLS.contains(&tool.as_str())),
        "{listed:?}"
    );
}

#[test]
fn refreshes_an_unedited_retired_seed_and_keeps_an_edited_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("coordinator.md");
    let old = "---\nname: coordinator\n---\nDo the work yourself.\n";
    fs::write(&path, old).unwrap();
    let hash = blake3::hash(old.as_bytes()).to_hex().to_string();
    let retired = [(COORDINATOR_DEFINITION, hash.as_str())];

    let refreshed = refresh_retired(dir.path(), &retired).unwrap();
    assert_eq!(refreshed, [COORDINATOR_DEFINITION]);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        seed_content(COORDINATOR_DEFINITION)
    );

    let edited = format!("{old}Keep it short.\n");
    fs::write(&path, &edited).unwrap();
    assert!(refresh_retired(dir.path(), &retired).unwrap().is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
}

#[test]
fn retired_seeds_name_built_ins_but_never_their_current_content() {
    for (name, hash) in RETIRED_SEEDS {
        let current = blake3::hash(seed_content(name).as_bytes()).to_hex();
        assert_eq!(hash.len(), 64, "{name}");
        assert_ne!(current.as_str(), *hash, "{name}'s current seed is retired");
    }
}

#[test]
fn system_prompt_includes_rules() {
    let def = AgentDefinition::parse(
        "---\nname: a\ndescription: Does A.\nskills: [missing]\n---\nRule one.\n",
        Path::new("a.md"),
    )
    .unwrap();
    let skills = SkillManager::new(aura_context_skills::SkillLoader::new(
        aura_context_skills::loader::SkillLoaderConfig::default(),
    ));
    let mut prompt = "base".to_string();
    def.extend_system_prompt(&mut prompt, &skills);
    assert!(prompt.starts_with("base\n\n<agent_definition name=\"a\">"));
    assert!(prompt.contains("Your role: Does A."));
    assert!(prompt.contains("Rule one."));
}

/// Characters per token, as the context breakdown estimates them.
const CHARS_PER_TOKEN: usize = aura_config::CHARS_PER_TOKEN;

/// A SKILL.md body of about `chars` characters: numbered steps, as a
/// real skill's instructions run.
fn skill_body(topic: &str, chars: usize) -> String {
    let mut body = format!("# {topic}\n\nFollow these steps in order.\n\n");
    let mut step = 1;
    while body.len() < chars {
        body.push_str(&format!(
            "{step}. For the {topic} work, check the inputs, run the matching command, read \
             its whole output, and write down what changed before moving on.\n"
        ));
        step += 1;
    }
    body
}

/// A manager with three skills of about 2,000, 3,500 and 6,000
/// characters, and a definition listing them.
fn three_skills(dir: &Path) -> (AgentDefinition, SkillManager) {
    let mut skills = SkillManager::new(aura_context_skills::SkillLoader::new(
        aura_context_skills::loader::SkillLoaderConfig {
            personal_dir: Some(dir.to_path_buf()),
            ..aura_context_skills::loader::SkillLoaderConfig::default()
        },
    ));
    for (name, chars) in [("deploy", 2_000), ("review", 3_500), ("migrate", 6_000)] {
        let description = format!("Use when asked to {name}.");
        skills
            .create(name, &description, &skill_body(name, chars), true)
            .unwrap();
    }
    let def = AgentDefinition::parse(
        "---\nname: builder\ndescription: Builds.\nskills: [deploy, review, migrate]\n---\nBe careful.\n",
        Path::new("builder.md"),
    )
    .unwrap();
    (def, skills)
}

#[test]
fn an_index_of_three_skills_costs_a_tenth_of_their_bodies_and_loads_each_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let (def, skills) = three_skills(dir.path());
    let mut full = String::from("base");
    def.extend_system_prompt_with(&mut full, &skills, SkillsIn::Full);
    let mut index = String::from("base");
    def.extend_system_prompt_with(&mut index, &skills, SkillsIn::Index);
    let (full_tokens, index_tokens) = (full.len() / CHARS_PER_TOKEN, index.len() / CHARS_PER_TOKEN);
    println!(
        "system prompt with 3 skills: {full_tokens} tokens in full, {index_tokens} as an index"
    );
    assert!(
        index_tokens * 10 < full_tokens,
        "the index ({index_tokens}) costs under a tenth of the bodies ({full_tokens})"
    );
    assert!(!index.contains("check the inputs"), "no body in the index");
    assert!(def.has_skills(&skills));
    for name in ["deploy", "review", "migrate"] {
        assert!(index.contains(&format!("name=\"{name}\"")));
        let loaded = def.load_skill(&skills, name, None).unwrap();
        assert!(loaded.contains(&skill_body(name, 0)[..10]), "{name}");
        assert!(loaded.len() > 1_900, "{name} in full");
    }
}

#[test]
fn a_definition_loads_only_its_own_skills() {
    let dir = tempfile::tempdir().unwrap();
    let (_, mut skills) = three_skills(dir.path());
    skills
        .create("other", "Not listed.", "Secret.", true)
        .unwrap();
    let def = AgentDefinition::parse(
        "---\nname: a\ndescription: A.\nskills: [deploy]\n---\n",
        Path::new("a.md"),
    )
    .unwrap();
    let error = def.load_skill(&skills, "other", None).unwrap_err();
    assert!(error.contains("your skills: deploy"), "{error}");
    let none =
        AgentDefinition::parse("---\nname: b\ndescription: B.\n---\n", Path::new("b.md")).unwrap();
    assert!(!none.has_skills(&skills));
}
