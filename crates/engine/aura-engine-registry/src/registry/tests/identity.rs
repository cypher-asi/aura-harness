use super::*;

#[test]
fn seeds_from_legacy_identity_file() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(LEGACY_IDENTITY_FILE), "0://terminal/legacy").unwrap();

    let registry = open(dir.path(), launch.path());

    assert_eq!(registry.agents().len(), 1);
    assert_eq!(registry.active().zns_id, "0://terminal/legacy");
    assert_eq!(registry.active().name, DEFAULT_AGENT_NAME);
    assert_eq!(
        registry.active().agent_id(),
        Identity::new("0://terminal/legacy", "x").agent_id
    );
    let project = registry.project_of(registry.active().agent_id()).unwrap();
    assert_eq!(project.folder, launch.path().canonicalize().unwrap());
    assert!(!registry.store().is_empty().unwrap());
}

#[test]
fn create_and_set_active_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let first = registry.active().clone();

    let named = registry.create(Some("  Researcher ".to_string())).unwrap();
    let unnamed = registry.create(None).unwrap();
    assert_eq!(named.name, "Researcher");
    assert_eq!(unnamed.name, "Agent 3");
    assert_eq!(named.project_id, first.project_id);
    registry.set_active(named.agent_id()).unwrap();

    let reloaded = open(dir.path(), launch.path());
    assert_eq!(reloaded.agents(), &[first, named.clone(), unnamed]);
    assert_eq!(reloaded.active(), &named);
    assert_eq!(reloaded.projects().len(), 1);
}

#[test]
fn headless_agent_is_registered_once_and_never_made_active() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let active = registry.active().clone();

    let headless = registry.headless().unwrap();
    assert_eq!(headless.name, HEADLESS_AGENT_NAME);
    assert_eq!(registry.headless().unwrap(), headless);

    let reloaded = open(dir.path(), launch.path());
    assert_eq!(reloaded.agents().len(), 2);
    assert_eq!(reloaded.active(), &active);
}

#[test]
fn loads_registry_written_before_definitions_existed() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join(REGISTRY_FILE),
        r#"{"agents":[{"zns_id":"0://terminal/a","name":"A","created_at":"2026-10-31T01:48:50Z"}]}"#,
    )
    .unwrap();

    let registry = open(dir.path(), launch.path());
    let agent = &registry.agents()[0];
    assert_eq!(agent.definition, None);
    assert_eq!(agent.parent, None);
    assert_eq!(
        agent.project_id.as_deref(),
        Some(hex("0://terminal/a").as_str())
    );
    assert_eq!(
        registry.project_of(agent.agent_id()).unwrap().folder,
        launch.path().canonicalize().unwrap()
    );
}

#[test]
fn cached_agent_id_is_the_identitys() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let agent = registry.create(Some("Named".to_string())).unwrap();
    assert_eq!(agent.agent_id(), agent.identity().agent_id);
    // A clone with another ZNS id must not keep the first one's id.
    let mut other = agent.clone();
    other.zns_id = "0://terminal/other".to_string();
    assert_eq!(
        other.agent_id(),
        Identity::new("0://terminal/other", "x").agent_id
    );
    assert_eq!(agent.agent_id(), agent.identity().agent_id);
}

#[test]
fn set_active_to_the_active_agent_does_not_rewrite_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let agent = registry.create(None).unwrap();
    registry.set_active(agent.agent_id()).unwrap();
    // Clear the stored active agent behind the registry's back, so a
    // write shows.
    let store = registry.store().clone();
    let stored_active = || store.load().unwrap().meta.unwrap().active;
    let mut meta = store.load().unwrap().meta.unwrap();
    meta.active = None;
    store
        .apply(&aura_store_db::agents::RegistryDiff {
            meta: Some(meta),
            ..Default::default()
        })
        .unwrap();
    registry.set_active(agent.agent_id()).unwrap();
    assert_eq!(
        stored_active(),
        None,
        "nothing changed, so nothing is written"
    );
    let first = registry.agents()[0].agent_id();
    registry.set_active(first).unwrap();
    assert_eq!(stored_active(), Some(first));
}
