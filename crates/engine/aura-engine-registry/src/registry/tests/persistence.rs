use super::*;

#[test]
fn importing_a_v3_agents_json_matches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let home = launch.path().canonicalize().unwrap();
    let away = other.path().canonicalize().unwrap();
    let file = serde_json::json!({
        "version": 3,
        "active": "0://terminal/re1",
        "projects": [
            {"id": "p-home", "folder": home, "created_at": "2026-01-02T03:04:05.123456789Z"},
            {"id": "p-away", "folder": away, "created_at": "2026-01-03T00:00:00Z", "removed": true},
        ],
        "agents": [
            {"zns_id": "0://terminal/main", "name": "Terminal Agent",
             "created_at": "2026-01-02T03:04:05Z", "project_id": "p-home", "code": "AGE1"},
            {"zns_id": "0://terminal/re1", "name": "rust-engineer #1",
             "created_at": "2026-01-02T04:00:00.5Z", "definition": "rust-engineer",
             "parent": "0://terminal/main", "project_id": "p-home", "code": "REN1",
             "subtitle": "fixing the build"},
            {"zns_id": "0://terminal/re2", "name": "rust-engineer #2",
             "created_at": "2026-01-02T05:00:00Z", "definition": "rust-engineer",
             "parent": "0://terminal/re1", "project_id": "p-home", "code": "REN2"},
            {"zns_id": "0://terminal/old", "name": "Agent 2",
             "created_at": "2026-01-02T06:00:00Z", "project_id": "p-home", "archived": true,
             "code": "AGE2"},
            {"zns_id": "0://terminal/docs", "name": "Agent 3",
             "created_at": "2026-01-02T07:00:00Z", "project_id": "p-home",
             "folder": away.join("docs"), "title": "Docs pass", "code": "AGE3"},
            {"zns_id": "0://terminal/away", "name": "Agent",
             "created_at": "2026-01-03T00:00:00Z", "project_id": "p-away", "archived": true,
             "code": "AGE4"},
        ],
        "prefixes": {"agent": "AGE", "rust-engineer": "REN", "reviewer": "REV"},
        "last_numbers": {"AGE": 7, "REN": 5},
    });
    let bytes = serde_json::to_vec_pretty(&file).unwrap();
    fs::write(dir.path().join(REGISTRY_FILE), &bytes).unwrap();

    let registry = open(dir.path(), launch.path());
    assert_matches_file(&registry, &file);
    assert!(!dir.path().join(REGISTRY_FILE).exists());
    assert_eq!(
        fs::read(dir.path().join(MIGRATED_REGISTRY_FILE)).unwrap(),
        bytes
    );
    let snapshot = registry.store().load().unwrap();
    drop(registry);

    // Read back from the store: the file is gone.
    let reloaded = open(dir.path(), launch.path());
    assert_matches_file(&reloaded, &file);
    assert_eq!(reloaded.store().load().unwrap(), snapshot);
    assert!(!dir.path().join(REGISTRY_FILE).exists());
}

#[test]
fn first_run_without_a_file_seeds_one_agent() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();

    let registry = open(dir.path(), launch.path());
    assert_eq!(registry.agents().len(), 1);
    let agent = registry.active().clone();
    assert_eq!(agent.name, DEFAULT_AGENT_NAME);
    assert!(
        agent.zns_id.starts_with("0://terminal/"),
        "{}",
        agent.zns_id
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(LEGACY_IDENTITY_FILE)).unwrap(),
        agent.zns_id
    );
    assert_eq!(
        registry.project_of(agent.agent_id()).unwrap().folder,
        launch.path().canonicalize().unwrap()
    );
    assert!(!dir.path().join(REGISTRY_FILE).exists());
    assert!(!dir.path().join(MIGRATED_REGISTRY_FILE).exists());
    drop(registry);

    let reloaded = open(dir.path(), launch.path());
    assert_eq!(reloaded.agents(), std::slice::from_ref(&agent));
    assert!(!dir.path().join(REGISTRY_FILE).exists());
}

#[test]
fn empty_store_beside_a_migrated_file_reimports_it_instead_of_seeding() {
    let launch = tempfile::tempdir().unwrap();
    let home = launch.path().canonicalize().unwrap();
    let file = serde_json::json!({
        "version": 3,
        "active": "0://terminal/re1",
        "projects": [
            {"id": "p-home", "folder": home, "created_at": "2026-01-02T03:04:05Z"},
        ],
        "agents": [
            {"zns_id": "0://terminal/main", "name": "Terminal Agent",
             "created_at": "2026-01-02T03:04:05Z", "project_id": "p-home", "code": "AGE1"},
            {"zns_id": "0://terminal/re1", "name": "rust-engineer #1",
             "created_at": "2026-01-02T04:00:00Z", "definition": "rust-engineer",
             "parent": "0://terminal/main", "project_id": "p-home", "code": "REN1"},
        ],
        "prefixes": {"agent": "AGE", "rust-engineer": "REN"},
        "last_numbers": {"AGE": 1, "REN": 1},
    });
    let bytes = serde_json::to_vec_pretty(&file).unwrap();
    // What a power loss after the rename leaves: the file renamed, the
    // store (a fresh data directory here) without the batch.
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(MIGRATED_REGISTRY_FILE), &bytes).unwrap();

    let registry = open(dir.path(), launch.path());
    assert_matches_file(&registry, &file);
    assert!(
        !dir.path().join(LEGACY_IDENTITY_FILE).exists(),
        "not seeded"
    );
    assert!(!dir.path().join(REGISTRY_FILE).exists());
    assert_eq!(
        fs::read(dir.path().join(MIGRATED_REGISTRY_FILE)).unwrap(),
        bytes,
        "the renamed file is left as it was"
    );
    let snapshot = registry.store().load().unwrap();
    drop(registry);

    let reloaded = open(dir.path(), launch.path());
    assert_matches_file(&reloaded, &file);
    assert_eq!(reloaded.store().load().unwrap(), snapshot);
}

#[test]
fn changes_survive_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let first = registry.active().clone();
    let home = first.project_id.clone().unwrap();
    let second = registry.create_in_project(&home).unwrap();
    let third = registry.create_in_project(&home).unwrap();
    let child = registry
        .create_instance("reviewer", Some(second.zns_id.clone()))
        .unwrap();
    let (away_agent, _) = registry.open_project(other.path()).unwrap();
    let away = away_agent.project_id.clone().unwrap();

    let reload = |registry: AgentRegistry, what: &str| {
        let before = state(&registry);
        drop(registry);
        let reloaded = open(dir.path(), launch.path());
        assert_eq!(state(&reloaded), before, "after {what}");
        reloaded
    };

    registry
        .reorder_agents(
            &home,
            &[
                third.agent_id().to_hex(),
                first.agent_id().to_hex(),
                second.agent_id().to_hex(),
            ],
        )
        .unwrap();
    let mut registry = reload(registry, "reorder_agents");
    let order: Vec<String> = registry
        .tree()
        .iter()
        .filter(|(a, _)| a.project_id.as_deref() == Some(home.as_str()))
        .map(|(a, _)| a.zns_id.clone())
        .collect();
    assert!(order.starts_with(&[third.zns_id.clone(), first.zns_id.clone()]));

    registry
        .reorder_projects(&[away.clone(), home.clone()])
        .unwrap();
    let mut registry = reload(registry, "reorder_projects");
    let ids: Vec<&str> = registry.projects().iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, [away.as_str(), home.as_str()]);

    registry.archive(second.agent_id()).unwrap();
    let mut registry = reload(registry, "archive");
    assert!(registry.find(second.agent_id()).unwrap().archived);
    assert!(registry
        .archived_tree()
        .iter()
        .any(|(a, _)| a.zns_id == child.zns_id));

    registry.restore(second.agent_id()).unwrap();
    let mut registry = reload(registry, "restore");
    assert!(!registry.find(second.agent_id()).unwrap().archived);

    registry.set_active(away_agent.agent_id()).unwrap();
    registry.remove_project(&away).unwrap();
    let registry = reload(registry, "remove_project");
    assert!(registry.project(&away).unwrap().removed);
    assert!(registry.find(away_agent.agent_id()).is_none());
    assert_eq!(registry.file.active, None);
}

#[test]
fn a_dangling_parent_survives_reloads_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            {"zns_id": "0://terminal/a", "name": "Agent", "created_at": "2026-10-31T01:48:50Z",
             "project_id": "p", "code": "AG1"},
            {"zns_id": "0://terminal/orphan", "name": "reviewer #1",
             "created_at": "2026-10-31T01:48:50Z", "definition": "reviewer",
             "parent": "0://terminal/gone", "project_id": "p", "code": "REV1"},
        ],
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();
    let orphan = Identity::new("0://terminal/orphan", "x").agent_id;

    let registry = open(dir.path(), launch.path());
    assert!(registry.find(orphan).unwrap().parent.is_some());
    assert!(registry.find(orphan).unwrap().needs_subtitle());
    let snapshot = registry.store().load().unwrap();
    let body = stored_body(dir.path());
    drop(registry);

    for _ in 0..2 {
        let reloaded = open(dir.path(), launch.path());
        let agent = reloaded.find(orphan).unwrap();
        assert!(agent.parent.is_some());
        assert!(agent.needs_subtitle());
        assert_eq!(reloaded.store().load().unwrap(), snapshot);
        assert_eq!(stored_body(dir.path()), body);
    }
}
