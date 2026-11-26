use super::*;

#[test]
fn only_placeholder_named_agents_need_a_title() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let terminal = registry.active().clone();
    let (project, _) = registry.open_project(folder.path()).unwrap();
    let numbered = registry
        .create_in_project(project.project_id.as_deref().unwrap())
        .unwrap();
    let unnamed = registry.create(None).unwrap();
    let named = registry.create(Some("Researcher".to_string())).unwrap();
    let instance = registry.create_instance("reviewer", None).unwrap();
    let headless = registry.headless().unwrap();

    assert!(terminal.needs_title());
    assert!(project.needs_title());
    assert!(numbered.needs_title());
    assert!(unnamed.needs_title());
    assert!(!named.needs_title());
    assert!(!instance.needs_title());
    assert!(!headless.needs_title());
    assert!(!is_placeholder_name("Agent "));
    assert!(!is_placeholder_name("Agent Smith"));
    assert!(!is_placeholder_name("Agents 2"));
}

#[test]
fn titles_are_shown_and_kept_without_changing_the_identity() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let agent = registry.create(None).unwrap();
    assert_eq!(agent.display_name(), agent.name);

    registry.set_title(agent.agent_id(), "   ").unwrap();
    registry
        .set_title(agent.agent_id(), "  Task build issue ")
        .unwrap();

    let reloaded = open(dir.path(), launch.path());
    let stored = reloaded.find(agent.agent_id()).unwrap();
    assert_eq!(stored.title.as_deref(), Some("Task build issue"));
    assert_eq!(stored.display_name(), "Task build issue");
    assert_eq!(stored.name, agent.name);
    assert_eq!(stored.agent_id(), agent.agent_id());
    assert!(!stored.needs_title());
    let unknown = Identity::new("0://terminal/unknown", "Unknown").agent_id;
    assert!(registry.set_title(unknown, "Title").is_err());
}

#[test]
fn only_spawned_instances_need_a_subtitle() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let parent = registry.active().clone();
    let spawned = registry
        .create_instance("reviewer", Some(parent.zns_id.clone()))
        .unwrap();
    let top_level = registry.create_instance("reviewer", None).unwrap();
    let plain = registry.create(None).unwrap();
    let child = registry
        .create_child("gate-1", &parent.zns_id, None)
        .unwrap();

    assert!(spawned.needs_subtitle());
    assert!(!top_level.needs_subtitle());
    assert!(!plain.needs_subtitle());
    assert!(!parent.needs_subtitle());
    assert!(!child.needs_subtitle());
}

#[test]
fn subtitles_are_kept_and_shown_after_the_code() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let parent = registry.active().zns_id.clone();
    let instance = registry.create_instance("reviewer", Some(parent)).unwrap();
    let code = instance.code.clone().unwrap();
    assert_eq!(instance.display_name(), instance.name);

    registry.set_subtitle(instance.agent_id(), "  ").unwrap();
    registry
        .set_subtitle(instance.agent_id(), " Fix parser ")
        .unwrap();

    let reloaded = open(dir.path(), launch.path());
    let stored = reloaded.find(instance.agent_id()).unwrap();
    assert_eq!(stored.subtitle.as_deref(), Some("Fix parser"));
    assert_eq!(stored.display_name(), format!("{code}: Fix parser"));
    assert_eq!(stored.name, instance.name);
    assert!(!stored.needs_subtitle());
    let unknown = Identity::new("0://terminal/unknown", "Unknown").agent_id;
    assert!(registry.set_subtitle(unknown, "Fix parser").is_err());
}

#[test]
fn the_label_is_the_code_then_the_title_or_else_the_subtitle() {
    let mut agent = AgentRegistry::new_agent("reviewer #1".to_string());
    agent.definition = Some("reviewer".to_string());
    agent.parent = Some("0://terminal/parent".to_string());
    assert_eq!(agent.display_name(), "reviewer #1");
    agent.code = Some("REV1".to_string());
    assert_eq!(agent.display_name(), "reviewer #1");
    agent.subtitle = Some("Fix parser".to_string());
    assert_eq!(agent.display_name(), "REV1: Fix parser");
    // An explicit title says more than the subtitle, and keeps the code.
    agent.title = Some("Did not start before the app closed".to_string());
    assert_eq!(
        agent.display_name(),
        "REV1: Did not start before the app closed"
    );
    agent.subtitle = None;
    assert_eq!(
        agent.display_name(),
        "REV1: Did not start before the app closed"
    );
    agent.code = None;
    assert_eq!(agent.display_name(), "Did not start before the app closed");
    // The label never becomes the identity.
    assert_eq!(agent.identity().name, "reviewer #1");

    // An agent with a subtitle is labelled by its code even when nothing
    // registered spawned it.
    let mut orphan = AgentRegistry::new_agent("reviewer #2".to_string());
    orphan.code = Some("REV2".to_string());
    orphan.subtitle = Some("Fix parser".to_string());
    assert_eq!(orphan.display_name(), "REV2: Fix parser");
    orphan.title = Some("Renamed".to_string());
    assert_eq!(orphan.display_name(), "REV2: Renamed");

    // Agents nobody spawned show their chat title alone; the panel has
    // the code in a column of its own.
    let mut top = AgentRegistry::new_agent("Agent 2".to_string());
    top.code = Some("AGE2".to_string());
    assert_eq!(top.display_name(), "Agent 2");
    top.title = Some("Task build issue".to_string());
    assert_eq!(top.display_name(), "Task build issue");
}

#[test]
fn a_title_set_first_is_not_replaced_by_a_late_subtitle() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let parent = registry.active().zns_id.clone();
    let instance = registry.create_instance("reviewer", Some(parent)).unwrap();
    assert!(instance.needs_subtitle());

    registry
        .set_title(instance.agent_id(), "Parser escapes, by hand")
        .unwrap();
    let stored = registry.find(instance.agent_id()).unwrap();
    assert!(!stored.needs_subtitle(), "a titled agent needs no subtitle");
    let code = code_of(stored).to_string();
    assert_eq!(
        stored.display_name(),
        format!("{code}: Parser escapes, by hand")
    );
}
