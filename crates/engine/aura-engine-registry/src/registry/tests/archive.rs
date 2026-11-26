use super::*;

#[test]
fn archived_agents_move_to_the_archive_with_what_they_spawned() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let root = registry.active().clone();
    let child = registry
        .create_instance("reviewer", Some(root.zns_id.clone()))
        .unwrap();
    let grandchild = registry
        .create_instance("explorer", Some(child.zns_id.clone()))
        .unwrap();
    registry.set_active(grandchild.agent_id()).unwrap();

    registry.archive(child.agent_id()).unwrap();
    let reloaded = open(dir.path(), launch.path());
    assert_eq!(names(reloaded.tree()), [(root.name.clone(), 0)]);
    assert_eq!(
        names(reloaded.archived_tree()),
        [(child.name.clone(), 0), (grandchild.name.clone(), 1)]
    );
    // The active agent was archived: the first listed one opens instead.
    assert_eq!(reloaded.active(), &root);

    // The last listed agent can't be archived.
    assert!(registry.archive(root.agent_id()).is_err());

    // Restoring a nested agent brings back the agents that spawned it.
    registry.archive(grandchild.agent_id()).unwrap();
    registry.restore(grandchild.agent_id()).unwrap();
    assert_eq!(registry.tree().len(), 3);
    assert!(registry.archived_tree().is_empty());
}

#[test]
fn the_archive_lists_the_newest_archived_first() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let [a, b, c, undated] = [(); 4].map(|()| registry.create(None).unwrap());
    // Out of registry order, so the dates decide.
    registry.archive_at(a.agent_id(), at(2)).unwrap();
    registry.archive_at(b.agent_id(), at(3)).unwrap();
    registry.archive_at(c.agent_id(), at(1)).unwrap();
    registry.archive_at(undated.agent_id(), at(4)).unwrap();
    // A registry written before the date was kept has no `archived_at`.
    let store = registry.store().clone();
    let mut row = store
        .load()
        .unwrap()
        .agents
        .into_iter()
        .find(|row| row.agent_id == undated.agent_id())
        .unwrap();
    row.archived_at = None;
    store
        .apply(&aura_store_db::agents::RegistryDiff {
            agents_put: vec![row],
            ..Default::default()
        })
        .unwrap();
    drop(store);
    drop(registry);

    let registry = open(dir.path(), launch.path());
    assert_eq!(registry.find(undated.agent_id()).unwrap().archived_at, None);
    assert_eq!(
        names(registry.archived_tree()),
        [(b.name, 0), (a.name, 0), (c.name, 0), (undated.name, 0)]
    );
}

#[test]
fn archiving_an_archived_agent_keeps_its_date_and_place() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let first = registry.create(None).unwrap();
    let second = registry.create(None).unwrap();
    registry.archive_at(first.agent_id(), at(1)).unwrap();
    registry.archive_at(second.agent_id(), at(2)).unwrap();

    registry.archive(first.agent_id()).unwrap();
    assert_eq!(
        registry.find(first.agent_id()).unwrap().archived_at,
        Some(at(1))
    );
    assert_eq!(
        names(registry.archived_tree()),
        [(second.name, 0), (first.name, 0)]
    );
}

#[test]
fn restoring_clears_the_archive_date() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let agent = registry.create(None).unwrap();
    registry.archive_at(agent.agent_id(), at(1)).unwrap();

    registry.restore(agent.agent_id()).unwrap();
    assert_eq!(registry.find(agent.agent_id()).unwrap().archived_at, None);
    let reloaded = open(dir.path(), launch.path());
    let restored = reloaded.find(agent.agent_id()).unwrap();
    assert!(!restored.archived);
    assert_eq!(restored.archived_at, None);
}

#[test]
fn the_current_agent_may_be_archived_but_the_startup_agent_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let legacy = registry.active().clone();
    let agent = registry.create(None).unwrap();
    registry.set_active(agent.agent_id()).unwrap();
    // The legacy agent stays listed, so this one can be archived.
    registry.archive(agent.agent_id()).unwrap();

    // Opened from the Archive.
    registry.set_active(agent.agent_id()).unwrap();
    assert_eq!(registry.current().agent_id(), agent.agent_id());
    assert_ne!(registry.active().agent_id(), agent.agent_id());
    assert_eq!(registry.active(), &legacy);
    assert!(registry.find(agent.agent_id()).unwrap().archived);
}
