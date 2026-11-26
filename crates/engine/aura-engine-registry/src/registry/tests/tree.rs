use super::*;

#[test]
fn a_parent_cycle_lists_each_agent_once() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, parent: Option<&str>, archived: bool| {
        serde_json::json!({
            "zns_id": zns, "name": zns, "created_at": "2026-10-31T01:48:50Z",
            "parent": parent, "project_id": "p", "archived": archived,
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://a", Some("0://b"), true),
            agent("0://b", Some("0://a"), true),
            agent("0://c", None, false),
            agent("0://d", Some("0://c"), false),
        ],
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();
    let registry = open(dir.path(), launch.path());

    assert_eq!(
        names(registry.tree()),
        [("0://c".to_string(), 0), ("0://d".to_string(), 1)]
    );
    // Both cycle members have an archived spawner, so neither heads the
    // Archive; the walk ends instead of looping.
    assert!(registry.archived_tree().len() <= registry.agents().len());
    let c = registry.agents()[2].agent_id();
    assert_eq!(registry.subtree(c).len(), 2);
    let a = registry.agents()[0].agent_id();
    assert_eq!(registry.subtree(a).len(), 2);
    assert_eq!(registry.project_counts("p").0, 2);
    assert_eq!(registry.project_counts_all().get("p").map(|c| c.0), Some(2));
}

#[test]
fn instances_are_numbered_per_definition_and_nested_under_parents() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let root = registry.active().clone();
    let other = registry.create(Some("Other".to_string())).unwrap();
    let child = registry
        .create_instance("reviewer", Some(root.zns_id.clone()))
        .unwrap();
    let grandchild = registry
        .create_instance("explorer", Some(child.zns_id.clone()))
        .unwrap();
    let second = registry.create_instance("reviewer", None).unwrap();
    assert_eq!(child.name, "reviewer #1");
    assert_eq!(second.name, "reviewer #2");
    assert_eq!(child.definition.as_deref(), Some("reviewer"));
    assert_eq!(grandchild.project_id, root.project_id);
    assert_eq!(second.project_id, root.project_id);

    let reloaded = open(dir.path(), launch.path());
    assert_eq!(
        names(reloaded.tree()),
        [
            (root.name.clone(), 0),
            (child.name.clone(), 1),
            (grandchild.name.clone(), 2),
            (other.name.clone(), 0),
            (second.name.clone(), 0),
        ]
    );
}
