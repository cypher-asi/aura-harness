use super::*;

/// The registry in `dir`, launched from `launch`.
fn open(dir: &Path, launch: &Path) -> AgentRegistry {
    AgentRegistry::load_or_init_in(dir, launch).unwrap()
}

fn names(tree: Vec<(&RegisteredAgent, usize)>) -> Vec<(String, usize)> {
    tree.into_iter().map(|(a, d)| (a.name.clone(), d)).collect()
}

fn project_names(registry: &AgentRegistry) -> Vec<String> {
    registry
        .listed_projects()
        .iter()
        .map(|p| p.name())
        .collect()
}

fn hex(zns: &str) -> String {
    Identity::new(zns, "x").agent_id.to_hex()
}

/// The registry stored in `dir`'s database, as the JSON of the in-memory
/// registry it loads as.
fn stored_body(dir: &Path) -> String {
    let rocks = shared_store(&dir.join(DB_DIR)).unwrap();
    let snapshot = AgentStore::for_store(&rocks).load().unwrap();
    serde_json::to_string_pretty(&persist::from_snapshot(snapshot)).unwrap()
}

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

#[test]
fn open_project_is_named_after_its_folder_and_adds_a_default_agent() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("website");
    fs::create_dir(&folder).unwrap();
    let mut registry = open(dir.path(), launch.path());

    let (created, opened) = registry.open_project(&folder).unwrap();
    assert_eq!(opened, Opened::Created);
    assert_eq!(created.name, "Agent");
    assert!(created.parent.is_none());
    let project = registry.project_of(created.agent_id()).unwrap().clone();
    assert_eq!(project.name(), "website");
    assert_eq!(project.folder, folder.canonicalize().unwrap());

    let reloaded = open(dir.path(), launch.path());
    assert_eq!(reloaded.project(&project.id), Some(&project));
    assert_eq!(project_names(&reloaded)[1], "website");
}

#[test]
fn a_folder_is_only_ever_one_project() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let (first, _) = registry.open_project(folder.path()).unwrap();
    let projects = registry.projects().len();

    // The same folder, spelled another way, opens the same project.
    let again = folder.path().join(".");
    let (found, opened) = registry.open_project(&again).unwrap();
    assert_eq!(opened, Opened::Existing);
    assert_eq!(found, first);
    assert_eq!(registry.projects().len(), projects);
    assert_eq!(registry.agents().len(), 2);

    // The launch directory's project is a folder like any other.
    let (launch_agent, opened) = registry.open_project(launch.path()).unwrap();
    assert_eq!(opened, Opened::Existing);
    assert_eq!(&launch_agent, registry.active());
}

#[test]
fn opening_a_removed_folder_brings_its_project_back_with_a_new_agent() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let (site, _) = registry.open_project(folder.path()).unwrap();
    let kept = registry
        .create_in_project(site.project_id.as_deref().unwrap())
        .unwrap();
    registry.archive(kept.agent_id()).unwrap();
    let id = site.project_id.clone().unwrap();
    registry.remove_project(&id).unwrap();
    assert_eq!(project_names(&registry).len(), 1);

    let (fresh, opened) = registry.open_project(folder.path()).unwrap();
    assert_eq!(opened, Opened::Revived);
    assert_eq!(fresh.project_id.as_deref(), Some(id.as_str()));
    assert_eq!(registry.projects().len(), 2);
    let listed: Vec<String> = registry
        .tree()
        .iter()
        .filter(|(a, _)| a.project_id.as_deref() == Some(id.as_str()))
        .map(|(a, _)| a.name.clone())
        .collect();
    assert_eq!(listed, [fresh.name]);
    assert_eq!(names(registry.archived_tree()), [(kept.name, 0)]);
}

#[test]
fn open_project_rejects_a_missing_folder_or_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let before = registry.agents().len();
    assert!(registry.open_project(&dir.path().join("missing")).is_err());
    let file = dir.path().join("notes.txt");
    fs::write(&file, "").unwrap();
    assert!(registry.open_project(&file).is_err());
    assert_eq!(registry.agents().len(), before);
    assert_eq!(registry.projects().len(), 1);
}

#[test]
fn spawned_agents_use_their_projects_folder() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let legacy = registry.active().clone();
    let (root, _) = registry.open_project(folder.path()).unwrap();
    let child = registry
        .create_instance("reviewer", Some(root.zns_id.clone()))
        .unwrap();
    let grandchild = registry
        .create_instance("explorer", Some(child.zns_id.clone()))
        .unwrap();

    let canonical = folder.path().canonicalize().unwrap();
    assert_eq!(
        registry.project_folder(grandchild.agent_id()),
        Some(canonical)
    );
    assert_eq!(
        registry.project_of(grandchild.agent_id()),
        registry.project_of(root.agent_id())
    );
    assert!(child.folder.is_none());
    assert_eq!(
        registry.project_folder(legacy.agent_id()),
        Some(launch.path().canonicalize().unwrap())
    );
}

#[test]
fn create_in_project_lists_a_numbered_agent_at_the_top_of_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let legacy = registry.active().clone();
    let (root, _) = registry.open_project(folder.path()).unwrap();
    let project = root.project_id.clone().unwrap();
    let reviewer = registry
        .create_instance("reviewer", Some(root.zns_id.clone()))
        .unwrap();

    let added = registry.create_in_project(&project).unwrap();
    assert_eq!(added.name, "Agent 3");
    assert_eq!(added.parent, None);
    assert_eq!(added.project_id.as_deref(), Some(project.as_str()));
    assert_eq!(
        registry.project_folder(added.agent_id()),
        Some(folder.path().canonicalize().unwrap())
    );
    let spawned = registry
        .create_instance("explorer", Some(added.zns_id.clone()))
        .unwrap();
    let instance = registry.add_instance_to_project("tests", &project).unwrap();

    let reloaded = open(dir.path(), launch.path());
    assert_eq!(
        names(reloaded.tree()),
        [
            (legacy.name, 0),
            (root.name, 0),
            (reviewer.name, 1),
            (added.name, 0),
            (spawned.name, 1),
            (instance.name, 0),
        ]
    );
    assert!(registry.create_in_project("unknown").is_err());
    assert!(registry
        .add_instance_to_project("tests", "unknown")
        .is_err());
}

#[test]
fn archiving_a_projects_first_agent_leaves_the_rest_of_the_project_listed() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let legacy = registry.active().clone();
    let (first, _) = registry.open_project(folder.path()).unwrap();
    let project = first.project_id.clone().unwrap();
    let spawned = registry
        .create_instance("reviewer", Some(first.zns_id.clone()))
        .unwrap();
    let second = registry.create_in_project(&project).unwrap();

    registry.archive(first.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [(legacy.name.clone(), 0), (second.name.clone(), 0)]
    );
    assert_eq!(project_names(&registry).len(), 2);
    assert_eq!(
        names(registry.archived_tree()),
        [(first.name.clone(), 0), (spawned.name.clone(), 1)]
    );
    assert_eq!(registry.project_counts(&project), (1, 2));

    // With every agent archived the project stays listed, empty, until
    // it is removed.
    registry.archive(second.agent_id()).unwrap();
    assert_eq!(names(registry.tree()), [(legacy.name.clone(), 0)]);
    assert_eq!(project_names(&registry).len(), 2);
    assert_eq!(registry.project_counts(&project), (0, 3));
    registry.remove_project(&project).unwrap();
    assert_eq!(project_names(&registry).len(), 1);
    assert_eq!(registry.archived_tree().len(), 3);
}

#[test]
fn a_version_2_project_with_every_agent_archived_stays_off_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let (agent, _) = registry.open_project(folder.path()).unwrap();
    let project = agent.project_id.clone().unwrap();
    registry.archive(agent.agent_id()).unwrap();
    registry.file.version = 2;
    registry.persist().unwrap();

    // Before version 3 it was not shown, so it stays hidden until one of
    // its agents is restored.
    let mut registry = open(dir.path(), launch.path());
    assert_eq!(project_names(&registry).len(), 1);
    assert!(registry.project(&project).unwrap().removed);
    registry.restore(agent.agent_id()).unwrap();
    assert_eq!(project_names(&registry).len(), 2);
}

#[test]
fn unarchiving_an_agent_brings_back_only_that_agent() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let legacy = registry.active().clone();
    let (first, _) = registry.open_project(folder.path()).unwrap();
    let project = first.project_id.clone().unwrap();
    let second = registry.create_in_project(&project).unwrap();
    let third = registry.create_in_project(&project).unwrap();
    registry.archive(first.agent_id()).unwrap();
    registry.archive(second.agent_id()).unwrap();
    registry.archive(third.agent_id()).unwrap();
    assert_eq!(names(registry.tree()), [(legacy.name.clone(), 0)]);

    registry.restore(second.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [(legacy.name.clone(), 0), (second.name.clone(), 0)]
    );
    assert_eq!(
        names(registry.archived_tree()),
        [(first.name.clone(), 0), (third.name.clone(), 0)]
    );

    // It comes back in its old place.
    registry.restore(first.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [(legacy.name, 0), (first.name, 0), (second.name, 0)]
    );
}

#[test]
fn restoring_an_agent_whose_project_was_removed_puts_back_just_that_agent() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let legacy = registry.active().clone();
    let (site, _) = registry.open_project(folder.path()).unwrap();
    let project = site.project_id.clone().unwrap();
    let added = registry.create_in_project(&project).unwrap();
    let other = registry.create_in_project(&project).unwrap();
    let spawned = registry
        .create_instance("reviewer", Some(added.zns_id.clone()))
        .unwrap();
    // Archived under a listed agent: that agent is kept too.
    let nested = registry
        .create_instance("tests", Some(other.zns_id.clone()))
        .unwrap();
    registry.archive(added.agent_id()).unwrap();
    registry.archive(nested.agent_id()).unwrap();

    let gone = registry.remove_project(&project).unwrap();
    assert_eq!(gone, [site.agent_id(), other.agent_id()]);
    assert!(registry.find(site.agent_id()).is_none());
    assert!(registry.find(other.agent_id()).unwrap().removed);
    assert_eq!(names(registry.tree()), [(legacy.name.clone(), 0)]);
    assert_eq!(
        names(registry.archived_tree()),
        [
            (added.name.clone(), 0),
            (spawned.name.clone(), 1),
            (nested.name.clone(), 0)
        ]
    );

    // Survives a reload, then comes back in the same project, alone.
    let mut registry = open(dir.path(), launch.path());
    assert_eq!(registry.active(), &legacy);
    registry.restore(added.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [
            (legacy.name.clone(), 0),
            (added.name.clone(), 0),
            (spawned.name.clone(), 1)
        ]
    );
    assert_eq!(registry.project_of(added.agent_id()).unwrap().id, project);
    assert_eq!(
        registry.project_folder(spawned.agent_id()),
        Some(folder.path().canonicalize().unwrap())
    );
    assert_eq!(names(registry.archived_tree()), [(nested.name.clone(), 0)]);

    // A nested agent brings back the agent that spawned it.
    registry.restore(nested.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [
            (legacy.name.clone(), 0),
            (added.name.clone(), 0),
            (spawned.name.clone(), 1),
            (other.name.clone(), 0),
            (nested.name.clone(), 1)
        ]
    );
    assert!(registry.archived_tree().is_empty());
}

#[test]
fn removing_a_project_drops_its_agents_but_not_the_last_project() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let legacy = registry.active().clone();
    let (site, _) = registry.open_project(folder.path()).unwrap();
    let project = site.project_id.clone().unwrap();
    let child = registry.create_in_project(&project).unwrap();
    let listed = registry.create_in_project(&project).unwrap();
    registry.archive(child.agent_id()).unwrap();
    registry.set_active(site.agent_id()).unwrap();

    let removed = registry.remove_project(&project).unwrap();
    assert_eq!(removed, [site.agent_id(), listed.agent_id()]);
    let reloaded = open(dir.path(), launch.path());
    assert_eq!(names(reloaded.tree()), [(legacy.name.clone(), 0)]);
    assert_eq!(names(reloaded.archived_tree()), [(child.name.clone(), 0)]);
    assert!(reloaded.find(listed.agent_id()).is_none());
    assert!(reloaded.project(&project).unwrap().removed);
    assert_eq!(reloaded.active(), &legacy);
    let launch_project = legacy.project_id.unwrap();
    assert!(registry.remove_project(&launch_project).is_err());
    assert!(registry.remove_project("unknown").is_err());
}

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
fn a_version_0_registry_becomes_one_project_per_folder() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let site = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let site_path = site.path().display().to_string();
    let docs_path = docs.path().display().to_string();
    let body = format!(
        r#"{{"agents":[
            {{"zns_id":"0://terminal/term","name":"Terminal Agent","created_at":"2026-11-01T00:00:00Z"}},
            {{"zns_id":"0://terminal/site","name":"Agent","created_at":"2026-11-01T00:00:00Z","project":"Site","folder":{site_path:?}}},
            {{"zns_id":"0://terminal/added","name":"Agent 2","created_at":"2026-11-01T00:00:00Z","member_of":"0://terminal/site"}},
            {{"zns_id":"0://terminal/old","name":"Agent 3","created_at":"2026-11-01T00:00:00Z","parent":"0://terminal/site"}},
            {{"zns_id":"0://terminal/spawned","name":"reviewer #1","created_at":"2026-11-01T00:00:00Z","definition":"reviewer","parent":"0://terminal/added"}},
            {{"zns_id":"0://terminal/dup","name":"Agent","created_at":"2026-11-01T00:00:00Z","project":"Copy","folder":{site_path:?}}},
            {{"zns_id":"0://terminal/docs","name":"Agent","created_at":"2026-11-01T00:00:00Z","project":"Docs","folder":{docs_path:?},"removed":true}},
            {{"zns_id":"0://terminal/kept","name":"Agent 2","created_at":"2026-11-01T00:00:00Z","member_of":"0://terminal/docs","archived":true}}
        ]}}"#
    );
    fs::write(dir.path().join(REGISTRY_FILE), body).unwrap();
    // The desktop app had the site project before the terminal's.
    fs::write(
        dir.path().join(DESKTOP_TABS_FILE),
        format!(
            "{}\n{}\n",
            hex("0://terminal/site"),
            hex("0://terminal/term")
        ),
    )
    .unwrap();

    let registry = open(dir.path(), launch.path());
    let ids: Vec<&str> = registry.projects().iter().map(|p| p.id.as_str()).collect();
    let (site_id, term_id, docs_id) = (
        hex("0://terminal/site"),
        hex("0://terminal/term"),
        hex("0://terminal/docs"),
    );
    assert_eq!(ids, [site_id.as_str(), term_id.as_str(), docs_id.as_str()]);
    let project = |zns: &str| {
        registry
            .agents()
            .iter()
            .find(|a| a.zns_id == zns)
            .unwrap()
            .project_id
            .clone()
            .unwrap()
    };
    for zns in ["site", "added", "old", "spawned", "dup"] {
        assert_eq!(project(&format!("0://terminal/{zns}")), site_id, "{zns}");
    }
    assert_eq!(project("0://terminal/term"), term_id);
    assert_eq!(project("0://terminal/kept"), docs_id);
    assert_eq!(
        registry.project(&site_id).unwrap().folder,
        site.path().canonicalize().unwrap()
    );
    assert_eq!(
        registry.project(&term_id).unwrap().folder,
        launch.path().canonicalize().unwrap()
    );
    assert!(registry.project(&docs_id).unwrap().removed);
    assert!(registry.agents().iter().all(|a| a.folder.is_none()));
    // The agent that headed the removed project stays off the list when
    // the archived one is restored.
    assert!(
        registry
            .agents()
            .iter()
            .find(|a| a.zns_id == "0://terminal/docs")
            .unwrap()
            .removed
    );
    assert_eq!(
        names(registry.tree()),
        [
            ("Agent".to_string(), 0),
            ("Agent 2".to_string(), 0),
            ("reviewer #1".to_string(), 1),
            ("Agent 3".to_string(), 0),
            ("Agent".to_string(), 0),
            ("Terminal Agent".to_string(), 0),
        ]
    );
    assert_eq!(
        names(registry.archived_tree()),
        [("Agent 2".to_string(), 0)]
    );

    let body = stored_body(dir.path());
    assert!(!body.contains("member_of"), "{body}");
    assert!(body.contains("\"version\": 3"), "{body}");
}

#[test]
fn projects_and_agents_keep_the_order_they_are_given() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let term = registry.active().project_id.clone().unwrap();
    let (first, _) = registry.open_project(a.path()).unwrap();
    let pa = first.project_id.clone().unwrap();
    let (other, _) = registry.open_project(b.path()).unwrap();
    let pb = other.project_id.clone().unwrap();

    // A view that shows only two of the projects swaps just those.
    registry
        .reorder_projects(&[pb.clone(), term.clone()])
        .unwrap();
    let ids: Vec<String> = registry.projects().iter().map(|p| p.id.clone()).collect();
    assert_eq!(ids, [pb.clone(), pa.clone(), term.clone()]);

    let spawned = registry
        .create_instance("reviewer", Some(first.zns_id.clone()))
        .unwrap();
    let second = registry.create_in_project(&pa).unwrap();
    let third = registry.create_in_project(&pa).unwrap();
    registry
        .reorder_agents(&pa, &[third.agent_id().to_hex(), first.agent_id().to_hex()])
        .unwrap();

    let reloaded = open(dir.path(), launch.path());
    let ids: Vec<String> = reloaded.projects().iter().map(|p| p.id.clone()).collect();
    assert_eq!(ids, [pb, pa, term]);
    assert_eq!(
        names(reloaded.tree()),
        [
            (other.name.clone(), 0),
            (third.name.clone(), 0),
            (second.name.clone(), 0),
            (first.name.clone(), 0),
            (spawned.name.clone(), 1),
            (DEFAULT_AGENT_NAME.to_string(), 0),
        ]
    );
}

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
    top.code = Some("AG2".to_string());
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

#[test]
fn compare_runs_work_in_their_own_folder_under_the_comparison() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let (root, _) = registry.open_project(project.path()).unwrap();
    let compare = registry.create_compare(&root.zns_id).unwrap();
    let second = registry.create_compare(&root.zns_id).unwrap();
    let run = registry
        .create_child("gate-1", &compare.zns_id, Some(copy.path().to_path_buf()))
        .unwrap();
    let subtask = registry
        .create_instance("explorer", Some(run.zns_id.clone()))
        .unwrap();

    assert_eq!(compare.name, "Compare #1");
    assert_eq!(second.name, "Compare #2");
    assert!(compare.is_compare() && !run.is_compare());
    let canonical = project.path().canonicalize().unwrap();
    assert_eq!(registry.project_folder(compare.agent_id()), Some(canonical));
    assert_eq!(
        registry.project_folder(subtask.agent_id()).as_deref(),
        Some(copy.path()),
        "a run's subtasks work in its copy"
    );
    assert_eq!(
        registry.descendants(compare.agent_id()),
        [run.agent_id(), subtask.agent_id()]
    );
    assert_eq!(
        registry.ancestry(subtask.agent_id()),
        [
            subtask.agent_id(),
            run.agent_id(),
            compare.agent_id(),
            root.agent_id()
        ]
    );
    assert!(registry.under_compare(run.agent_id()));
    assert!(registry.under_compare(subtask.agent_id()));
    assert!(!registry.under_compare(compare.agent_id()));
    assert!(!registry.under_compare(root.agent_id()));

    let reloaded = open(dir.path(), launch.path());
    let names: Vec<&str> = reloaded
        .compares()
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(names, ["Compare #1", "Compare #2"]);
}

#[test]
fn compare_runs_list_by_run_number_then_arm() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let (root, _) = registry.open_project(project.path()).unwrap();
    let compare = registry.create_compare(&root.zns_id).unwrap();
    for name in ["b-1", "b-2", "a-1", "a-10", "b-10", "a-2"] {
        registry.create_child(name, &compare.zns_id, None).unwrap();
    }
    let a1 = registry
        .tree()
        .iter()
        .find(|(a, _)| a.name == "a-1")
        .unwrap()
        .0
        .clone();
    registry
        .create_instance("implement", Some(a1.zns_id.clone()))
        .unwrap();

    let names: Vec<&str> = registry
        .tree()
        .into_iter()
        .skip_while(|(a, _)| a.zns_id != compare.zns_id)
        .skip(1)
        .map(|(a, _)| a.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["a-1", "implement #1", "b-1", "a-2", "b-2", "a-10", "b-10"]
    );
}

#[test]
fn each_arm_of_a_compare_lists_its_runs_under_an_agent_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let (root, _) = registry.open_project(project.path()).unwrap();
    let compare = registry.create_compare(&root.zns_id).unwrap();
    let a = registry
        .create_compare_arm("A · gate", &compare.zns_id)
        .unwrap();
    let b = registry
        .create_compare_arm("B · claude-opus-5-5", &compare.zns_id)
        .unwrap();
    for (name, arm) in [("b-2", &b), ("a-2", &a), ("b-1", &b), ("a-1", &a)] {
        registry.create_child(name, &arm.zns_id, None).unwrap();
    }

    let rows: Vec<(String, usize)> = registry
        .tree()
        .into_iter()
        .skip_while(|(agent, _)| agent.zns_id != compare.zns_id)
        .map(|(agent, depth)| (agent.name.clone(), depth))
        .collect();
    let d = rows[0].1;
    let expected = [
        ("Compare #1", d),
        ("A · gate", d + 1),
        ("a-1", d + 2),
        ("a-2", d + 2),
        ("B · claude-opus-5-5", d + 1),
        ("b-1", d + 2),
        ("b-2", d + 2),
    ]
    .map(|(name, depth)| (name.to_string(), depth));
    assert_eq!(rows, expected);
    assert!(a.is_compare_arm() && !a.is_compare());
    assert!(!registry.under_compare(a.agent_id()), "an arm does no work");
    let run = registry
        .tree()
        .iter()
        .find(|(r, _)| r.name == "b-1")
        .unwrap()
        .0
        .agent_id();
    assert!(registry.under_compare(run));
}

/// The code of `agent`, which every registered agent has.
fn code_of(agent: &RegisteredAgent) -> &str {
    agent
        .code
        .as_deref()
        .unwrap_or_else(|| panic!("{} has no code", agent.name))
}

/// Every agent's ZNS id with its code, in registry order.
fn codes(registry: &AgentRegistry) -> Vec<(String, Option<String>)> {
    registry
        .agents()
        .iter()
        .map(|a| (a.zns_id.clone(), a.code.clone()))
        .collect()
}

/// The prefix and number of `code`, which must be `<PREFIX><N>`.
fn split_code(code: &str) -> (&str, u64) {
    let digits = code.find(|c: char| c.is_ascii_digit()).unwrap();
    let (prefix, number) = code.split_at(digits);
    assert!(!number.starts_with('0'), "{code}");
    (prefix, number.parse().unwrap())
}

/// Checks the code invariants over every agent of `registry`: each has a
/// `<PREFIX><N>` code with a prefix of two or three uppercase letters, no
/// two share one, all agents of a source key share a prefix and no two
/// keys do.
fn assert_codes_consistent(registry: &AgentRegistry) {
    let mut seen = HashSet::new();
    let mut prefix_of_key: HashMap<String, String> = HashMap::new();
    for agent in registry.agents() {
        let code = code_of(agent);
        assert!(seen.insert(code.to_string()), "{code} given twice");
        let (prefix, number) = split_code(code);
        assert!((2..=3).contains(&prefix.len()), "{code}");
        assert!(prefix.bytes().all(|b| b.is_ascii_uppercase()), "{code}");
        assert!(number > 0, "{code}");
        let key = agent.definition.clone().unwrap_or_else(|| "agent".into());
        let known = prefix_of_key
            .entry(key.clone())
            .or_insert_with(|| prefix.to_string());
        assert_eq!(known, prefix, "{key} has two prefixes");
    }
    let prefixes: HashSet<&String> = prefix_of_key.values().collect();
    assert_eq!(prefixes.len(), prefix_of_key.len(), "{prefix_of_key:?}");
}

#[test]
fn instance_codes_are_numbered_like_their_names() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    for n in 1..=3 {
        let agent = registry.create_instance("rust-engineer", None).unwrap();
        assert_eq!(agent.name, format!("rust-engineer #{n}"));
        assert_eq!(code_of(&agent), format!("RE{n}"));
        let (_, name_number) = agent.name.rsplit_once('#').unwrap();
        let (_, code_number) = split_code(code_of(&agent));
        assert_eq!(code_number.to_string(), name_number);
        // What was registered is what was returned.
        assert_eq!(registry.find(agent.agent_id()).unwrap().code, agent.code);
    }
}

#[test]
fn a_one_word_definition_takes_its_first_two_letters() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let explorer = registry.create_instance("explorer", None).unwrap();
    assert_eq!(code_of(&explorer), "EX1");
}

#[test]
fn a_definition_whose_prefix_is_taken_uses_the_next_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let engineer = registry.create_instance("rust-engineer", None).unwrap();
    let reviewer = registry.create_instance("reviewer", None).unwrap();
    let second = registry.create_instance("reviewer", None).unwrap();
    let engineer_2 = registry.create_instance("rust-engineer", None).unwrap();
    // `RE` is taken: the first word's start and the second's initial.
    let research = registry.create_instance("research-engineer", None).unwrap();
    // `re` has no candidate of its own but `RE`: `R` then `AA`.
    let re = registry.create_instance("re", None).unwrap();

    assert_eq!(code_of(&engineer), "RE1");
    assert_eq!(code_of(&reviewer), "REV1");
    assert_eq!(code_of(&second), "REV2");
    assert_eq!(code_of(&engineer_2), "RE2");
    assert_eq!(code_of(&research), "REE1");
    assert_eq!(code_of(&re), "RAA1");
    assert_codes_consistent(&registry);
}

#[test]
fn agents_without_a_definition_are_coded_ag() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let terminal = registry.active().clone();
    let unnamed = registry.create(None).unwrap();
    let instance = registry.create_instance("rust-engineer", None).unwrap();
    let named = registry.create(Some("Researcher".to_string())).unwrap();
    let (project, _) = registry.open_project(folder.path()).unwrap();
    let in_project = registry
        .create_in_project(project.project_id.as_deref().unwrap())
        .unwrap();
    let child = registry
        .create_child("gate-1", &project.zns_id, None)
        .unwrap();

    assert_eq!(code_of(&instance), "RE1");
    let generic: Vec<&str> = [&terminal, &unnamed, &named, &project, &in_project, &child]
        .into_iter()
        .map(code_of)
        .collect();
    assert_eq!(generic, ["AG1", "AG2", "AG3", "AG4", "AG5", "AG6"]);
}

#[test]
fn every_way_of_creating_an_agent_gives_a_unique_code() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let root = registry.active().clone();
    let (site, _) = registry.open_project(folder.path()).unwrap();
    let project = site.project_id.clone().unwrap();
    let engineer = registry
        .create_instance("rust-engineer", Some(root.zns_id.clone()))
        .unwrap();
    registry
        .create_instance("explorer", Some(engineer.zns_id.clone()))
        .unwrap();
    registry
        .add_instance_to_project("rust-engineer", &project)
        .unwrap();
    registry
        .add_instance_to_project("reviewer", &project)
        .unwrap();
    registry.create_instance("reviewer", None).unwrap();
    registry.create_in_project(&project).unwrap();
    registry.create(None).unwrap();
    registry.headless().unwrap();
    let compare = registry.create_compare(&site.zns_id).unwrap();
    let arm = registry
        .create_compare_arm("A · gate", &compare.zns_id)
        .unwrap();
    let run = registry.create_child("a-1", &arm.zns_id, None).unwrap();
    registry
        .create_instance("explorer", Some(run.zns_id.clone()))
        .unwrap();
    registry.create_compare(&site.zns_id).unwrap();
    registry.create_instance("research-engineer", None).unwrap();
    registry.create_instance("tests", None).unwrap();

    assert_eq!(registry.agents().len(), 17);
    assert_codes_consistent(&registry);
    assert_codes_consistent(&open(dir.path(), launch.path()));
}

#[test]
fn a_dropped_instance_number_is_never_handed_out_again() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let (site, _) = registry.open_project(folder.path()).unwrap();
    let project = site.project_id.clone().unwrap();
    let first = registry
        .add_instance_to_project("rust-engineer", &project)
        .unwrap();
    let second = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!((code_of(&first), code_of(&second)), ("RE1", "RE2"));

    // Dropping `#1` leaves `#2`, so the next instance is `#3`: names stay
    // unique and the code still matches the name's number.
    registry.remove_project(&project).unwrap();
    let third = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(third.name, "rust-engineer #3");
    assert_eq!(code_of(&third), "RE3");

    // Dropping the highest, `#3`, does not free its number either.
    registry.forget(third.agent_id()).unwrap();
    let fourth = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(fourth.name, "rust-engineer #4");
    assert_eq!(code_of(&fourth), "RE4");
    assert_codes_consistent(&registry);
}

#[test]
fn forgetting_the_highest_instance_does_not_free_its_number() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    registry.create_instance("rust-engineer", None).unwrap();
    let second = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(code_of(&second), "RE2");

    registry.forget(second.agent_id()).unwrap();
    let next = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(next.name, "rust-engineer #3");
    assert_eq!(code_of(&next), "RE3");
    assert_codes_consistent(&registry);
}

#[test]
fn removing_the_project_of_the_highest_instance_does_not_free_its_number() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    registry.create_instance("rust-engineer", None).unwrap();
    let (site, _) = registry.open_project(folder.path()).unwrap();
    let project = site.project_id.clone().unwrap();
    let highest = registry
        .add_instance_to_project("rust-engineer", &project)
        .unwrap();
    assert_eq!(code_of(&highest), "RE2");

    registry.remove_project(&project).unwrap();
    assert!(registry.find(highest.agent_id()).is_none());
    let next = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(next.name, "rust-engineer #3");
    assert_eq!(code_of(&next), "RE3");
    assert_codes_consistent(&registry);
}

#[test]
fn a_forgotten_generic_agents_code_is_never_handed_out_again() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let root = registry.active().clone();
    assert_eq!(code_of(&root), "AG1");
    // As `/compare` registers a run and forgets it when it never starts.
    let run = registry.create_child("a-1", &root.zns_id, None).unwrap();
    assert_eq!(code_of(&run), "AG2");

    registry.forget(run.agent_id()).unwrap();
    let next = registry.create_child("a-1", &root.zns_id, None).unwrap();
    assert_eq!(code_of(&next), "AG3");
    assert_codes_consistent(&registry);
}

#[test]
fn the_highest_number_given_survives_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let root = registry.active().clone();
    registry.create_instance("rust-engineer", None).unwrap();
    let second = registry.create_instance("rust-engineer", None).unwrap();
    let run = registry.create_child("a-1", &root.zns_id, None).unwrap();
    registry.forget(second.agent_id()).unwrap();
    registry.forget(run.agent_id()).unwrap();

    let mut reloaded = open(dir.path(), launch.path());
    let next = reloaded.create_instance("rust-engineer", None).unwrap();
    assert_eq!(next.name, "rust-engineer #3");
    assert_eq!(code_of(&next), "RE3");
    let child = reloaded.create_child("a-1", &root.zns_id, None).unwrap();
    assert_eq!(code_of(&child), "AG3");
    assert_eq!(
        saved_last_numbers(dir.path()),
        serde_json::json!({"AG": 3, "RE": 3})
    );
}

#[test]
fn loading_a_registry_without_last_numbers_seeds_them_from_its_codes() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str, definition: Option<&str>, code: Option<&str>| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": definition, "project_id": "p", "code": code,
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://terminal/agent", "Agent", None, Some("AG1")),
            agent("0://terminal/re1", "rust-engineer #1", Some("rust-engineer"), Some("RE1")),
            agent("0://terminal/re2", "rust-engineer #2", Some("rust-engineer"), Some("RE2")),
            // Written before codes: backfilled, and counted.
            agent("0://terminal/re5", "rust-engineer #5", Some("rust-engineer"), None),
        ],
        "prefixes": {"agent": "AG", "rust-engineer": "RE"},
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();

    let mut registry = open(dir.path(), launch.path());
    assert_eq!(code_of(&registry.agents()[3]), "RE5");
    assert_eq!(
        saved_last_numbers(dir.path()),
        serde_json::json!({"AG": 1, "RE": 5})
    );
    let highest = registry.agents()[3].agent_id();
    registry.forget(highest).unwrap();
    let next = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(next.name, "rust-engineer #6");
    assert_eq!(code_of(&next), "RE6");
}

#[test]
fn backfilled_instances_keep_their_numbers_in_any_order() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": "rust-engineer", "project_id": "p",
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        // Reordered in the list, so the higher number comes first.
        "agents": [
            agent("0://terminal/re5", "rust-engineer #5"),
            agent("0://terminal/re3", "rust-engineer #3"),
        ],
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();

    let mut registry = open(dir.path(), launch.path());
    let got: Vec<&str> = registry.agents().iter().map(code_of).collect();
    assert_eq!(got, ["RE5", "RE3"]);
    let next = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(
        (next.name.as_str(), code_of(&next)),
        ("rust-engineer #6", "RE6")
    );
}

/// The highest numbers given, as saved in `dir`'s registry file.
fn saved_last_numbers(dir: &Path) -> serde_json::Value {
    let body = stored_body(dir);
    let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
    saved["last_numbers"].clone()
}

#[test]
fn reopening_the_registry_keeps_every_code() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let root = registry.active().clone();
    let (site, _) = registry.open_project(folder.path()).unwrap();
    let engineer = registry
        .create_instance("rust-engineer", Some(root.zns_id.clone()))
        .unwrap();
    registry
        .create_instance("reviewer", Some(engineer.zns_id.clone()))
        .unwrap();
    let archived = registry
        .add_instance_to_project("explorer", site.project_id.as_deref().unwrap())
        .unwrap();
    registry.create(None).unwrap();
    registry.headless().unwrap();
    registry.archive(archived.agent_id()).unwrap();
    let before = codes(&registry);
    assert!(before.iter().all(|(_, code)| code.is_some()));

    let reloaded = open(dir.path(), launch.path());
    assert_eq!(codes(&reloaded), before);
    assert_eq!(codes(&open(dir.path(), launch.path())), before);
}

#[test]
fn a_registry_without_codes_gets_them_on_load_and_keeps_them() {
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str, definition: Option<&str>| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": definition, "project_id": "p",
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://terminal/agent", "Agent", None),
            agent("0://terminal/re36", "rust-engineer #36", Some("rust-engineer")),
            agent("0://terminal/rev", "reviewer #1", Some("reviewer")),
            agent("0://terminal/agent2", "Agent 2", None),
        ],
    })
    .to_string();
    assert!(!file.contains("\"code\""), "{file}");

    let load = || {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(REGISTRY_FILE), &file).unwrap();
        let registry = open(dir.path(), launch.path());
        (dir, codes(&registry))
    };
    let (dir, first) = load();
    let (_other, second) = load();
    let expected = [
        ("0://terminal/agent", "AG1"),
        ("0://terminal/re36", "RE36"),
        ("0://terminal/rev", "REV1"),
        ("0://terminal/agent2", "AG2"),
    ]
    .map(|(zns, code)| (zns.to_string(), Some(code.to_string())));
    assert_eq!(first, expected);
    assert_eq!(second, first, "the same registry backfills the same codes");

    let body = stored_body(dir.path());
    let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
    let saved: Vec<&str> = saved["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["code"].as_str().unwrap())
        .collect();
    assert_eq!(saved, ["AG1", "RE36", "REV1", "AG2"]);
    assert_eq!(codes(&open(dir.path(), launch.path())), first);
}

#[test]
fn backfilling_keeps_the_codes_a_registry_has() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str, definition: Option<&str>, code: Option<&str>| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": definition, "project_id": "p", "code": code,
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://terminal/re36", "rust-engineer #36", Some("rust-engineer"), None),
            agent("0://terminal/rev", "reviewer #1", Some("reviewer"), Some("RE1")),
            agent("0://terminal/agent", "Agent", None, Some("AG5")),
            agent("0://terminal/agent2", "Agent 2", None, None),
            agent("0://terminal/rev2", "reviewer #2", Some("reviewer"), None),
        ],
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();

    let registry = open(dir.path(), launch.path());
    let got: Vec<&str> = registry.agents().iter().map(code_of).collect();
    // `RE` is the reviewer's, coded later in the list but coded already,
    // so the engineer takes its first three-letter candidate.
    assert_eq!(got, ["RUE36", "RE1", "AG5", "AG6", "RE2"]);
    assert_codes_consistent(&registry);
}

#[test]
fn older_codes_and_prefixes_are_migrated_on_load_and_kept() {
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str, definition: Option<&str>, code: &str| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": definition, "project_id": "p", "code": code,
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://terminal/agent", "Agent", None, "AG-1"),
            agent("0://terminal/re36", "rust-engineer #36", Some("rust-engineer"), "RE-36"),
            agent("0://terminal/rev", "reviewer #1", Some("reviewer"), "REVI-1"),
            agent("0://terminal/res", "research-engineer #1", Some("research-engineer"), "RE2-1"),
            // Hand-edited to the code `RE-36` becomes: it is dropped.
            agent("0://terminal/re37", "rust-engineer #37", Some("rust-engineer"), "RE36"),
        ],
        "prefixes": {
            "agent": "AG", "rust-engineer": "RE", "reviewer": "REVI", "research-engineer": "RE2",
        },
    })
    .to_string();

    let load = || {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(REGISTRY_FILE), &file).unwrap();
        let registry = open(dir.path(), launch.path());
        (dir, codes(&registry))
    };
    let (dir, first) = load();
    let (_other, second) = load();
    let expected = [
        ("0://terminal/agent", "AG1"),
        ("0://terminal/re36", "RE36"),
        ("0://terminal/rev", "REV1"),
        ("0://terminal/res", "REE1"),
        ("0://terminal/re37", "RE37"),
    ]
    .map(|(zns, code)| (zns.to_string(), Some(code.to_string())));
    assert_eq!(first, expected);
    assert_eq!(second, first, "the same registry migrates the same way");
    assert_eq!(
        saved_prefixes(dir.path()),
        serde_json::json!({
            "agent": "AG", "rust-engineer": "RE", "reviewer": "REV", "research-engineer": "REE",
        })
    );

    // Saved migrated, so loading again changes nothing.
    let body = stored_body(dir.path());
    let reloaded = open(dir.path(), launch.path());
    assert_eq!(codes(&reloaded), first);
    assert_codes_consistent(&reloaded);
    assert_eq!(stored_body(dir.path()), body);
}

#[test]
fn codes_stay_valid_and_unique_over_many_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let words = [
        "rust", "research", "review", "re", "engineer", "explorer", "e", "agent", "x", "test",
    ];
    let keys: Vec<String> = (0..100)
        .map(|i| match (words[i / 10], words[i % 10]) {
            (a, b) if a == b => a.to_string(),
            (a, b) => format!("{a}-{b}"),
        })
        .collect();
    for key in &keys {
        registry.create_instance(key, None).unwrap();
    }
    for key in &keys[..10] {
        registry.create_instance(key, None).unwrap();
    }
    assert_eq!(registry.agents().len(), 1 + 110);
    assert_codes_consistent(&registry);
    let reloaded = open(dir.path(), launch.path());
    assert_eq!(codes(&reloaded), codes(&registry));
}

/// The prefix table as saved in `dir`'s registry file.
fn saved_prefixes(dir: &Path) -> serde_json::Value {
    let body = stored_body(dir);
    let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
    saved["prefixes"].clone()
}

#[test]
fn definitions_reserve_their_prefix_before_any_instance_exists() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let keys = ["rust-engineer", "reviewer", "explorer"];
    assert_eq!(
        registry.reserve_prefixes(&keys).unwrap(),
        ["RE", "REV", "EX"]
    );
    assert_eq!(
        saved_prefixes(dir.path()),
        serde_json::json!({
            "agent": "AG", "rust-engineer": "RE", "reviewer": "REV", "explorer": "EX",
        })
    );

    let engineer = registry.create_instance("rust-engineer", None).unwrap();
    let reviewer = registry.create_instance("reviewer", None).unwrap();
    assert_eq!((code_of(&engineer), code_of(&reviewer)), ("RE1", "REV1"));

    let mut reloaded = open(dir.path(), launch.path());
    let body = stored_body(dir.path());
    assert_eq!(
        reloaded.reserve_prefixes(&keys).unwrap(),
        ["RE", "REV", "EX"]
    );
    let explorer = reloaded.create_instance("explorer", None).unwrap();
    assert_eq!(code_of(&explorer), "EX1");
    assert_codes_consistent(&reloaded);
    // Asking again reserves nothing new, so the file is left alone.
    let after = stored_body(dir.path());
    assert_ne!(after, body, "creating the explorer saved");
    reloaded.reserve_prefixes(&keys).unwrap();
    assert_eq!(stored_body(dir.path()), after);
}

#[test]
fn prefixes_are_reserved_in_the_order_asked() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    assert_eq!(
        registry
            .reserve_prefixes(&["reviewer", "rust-engineer"])
            .unwrap(),
        ["RE", "RUE"]
    );
    let engineer = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(code_of(&engineer), "RUE1");
}

#[test]
fn a_registry_without_a_prefix_table_rebuilds_it_from_its_codes() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str, definition: Option<&str>, code: &str| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": definition, "project_id": "p", "code": code,
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://terminal/rev", "reviewer #1", Some("reviewer"), "RE1"),
            agent("0://terminal/re36", "rust-engineer #36", Some("rust-engineer"), "RUS36"),
            agent("0://terminal/agent", "Agent", None, "AG5"),
            agent("0://terminal/rev2", "reviewer #2", Some("reviewer"), "RE2"),
            // Hand-edited: `RE` is the reviewer's, so this code is dropped
            // and the key gets a prefix of its own.
            agent("0://terminal/re", "re #1", Some("re"), "RE7"),
        ],
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();

    let mut registry = open(dir.path(), launch.path());
    assert_eq!(
        saved_prefixes(dir.path()),
        serde_json::json!({"reviewer": "RE", "rust-engineer": "RUS", "agent": "AG", "re": "RAA"})
    );
    assert_eq!(code_of(&registry.agents()[4]), "RAA1");
    assert_codes_consistent(&registry);
    let keys = ["rust-engineer", "reviewer", "agent", "explorer", "re"];
    assert_eq!(
        registry.reserve_prefixes(&keys).unwrap(),
        ["RUS", "RE", "AG", "EX", "RAA"]
    );
    let engineer = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(code_of(&engineer), "RUS37");
    let reviewer = registry.create_instance("reviewer", None).unwrap();
    assert_eq!(code_of(&reviewer), "RE3");
}

#[test]
fn reserved_prefixes_are_unique_across_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let keys = [
        "re",
        "research-engineer",
        "reviewer",
        "rust-engineer",
        "rust_engineer",
        "explorer",
        "explore",
        "ex",
        "e",
        "frontend-engineer",
        "agent",
        "--",
        "tests",
        "test",
    ];
    let prefixes = registry.reserve_prefixes(&keys).unwrap();
    let unique: HashSet<&String> = prefixes.iter().collect();
    assert_eq!(unique.len(), keys.len(), "{prefixes:?}");
    assert_eq!(prefixes[..3], ["RE", "REE", "REV"]);

    let mut reloaded = open(dir.path(), launch.path());
    assert_eq!(reloaded.reserve_prefixes(&keys).unwrap(), prefixes);
    // A key asked twice gets the same prefix both times.
    assert_eq!(
        reloaded
            .reserve_prefixes(&["reviewer", "reviewer"])
            .unwrap(),
        ["REV", "REV"]
    );
    for (key, prefix) in keys
        .iter()
        .zip(&prefixes)
        .filter(|(key, _)| **key != "agent")
    {
        let instance = reloaded.create_instance(key, None).unwrap();
        assert_eq!(code_of(&instance), format!("{prefix}1"), "{key}");
    }
    assert_codes_consistent(&reloaded);
}

#[test]
fn codes_serialize_only_when_present() {
    let mut agent = AgentRegistry::new_agent("rust-engineer #36".to_string());
    agent.definition = Some("rust-engineer".to_string());
    let json = serde_json::to_value(&agent).unwrap();
    assert!(json.get("code").is_none(), "{json}");
    let back: RegisteredAgent = serde_json::from_value(json).unwrap();
    assert_eq!(back.code, None);

    agent.code = Some("RE36".to_string());
    let json = serde_json::to_string(&agent).unwrap();
    assert!(json.contains(r#""code":"RE36""#), "{json}");
    let back: RegisteredAgent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, agent);
    assert_eq!(back.code.as_deref(), Some("RE36"));
}

/// `value[key]` as a string, `None` when absent or null.
fn json_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// `value[key]` as a flag, `false` when absent.
fn json_flag(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn json_time(value: &serde_json::Value) -> DateTime<Utc> {
    value["created_at"].as_str().unwrap().parse().unwrap()
}

/// Assert, field by field, that `registry` holds what the version 3
/// registry file `file` does, in its order.
fn assert_matches_file(registry: &AgentRegistry, file: &serde_json::Value) {
    let agents = file["agents"].as_array().unwrap();
    let zns: Vec<&str> = registry
        .agents()
        .iter()
        .map(|a| a.zns_id.as_str())
        .collect();
    let want: Vec<&str> = agents
        .iter()
        .map(|a| a["zns_id"].as_str().unwrap())
        .collect();
    assert_eq!(zns, want, "same agents in the same order");
    for (agent, want) in registry.agents().iter().zip(agents) {
        let id = &agent.zns_id;
        assert_eq!(Some(agent.name.clone()), json_str(want, "name"), "{id}");
        assert_eq!(agent.created_at, json_time(want), "{id}");
        assert_eq!(agent.definition, json_str(want, "definition"), "{id}");
        assert_eq!(agent.parent, json_str(want, "parent"), "{id}");
        assert_eq!(agent.project_id, json_str(want, "project_id"), "{id}");
        assert_eq!(
            agent.folder,
            json_str(want, "folder").map(PathBuf::from),
            "{id}"
        );
        assert_eq!(agent.archived, json_flag(want, "archived"), "{id}");
        assert_eq!(agent.removed, json_flag(want, "removed"), "{id}");
        assert_eq!(agent.title, json_str(want, "title"), "{id}");
        assert_eq!(agent.code, json_str(want, "code"), "{id}");
        assert_eq!(agent.subtitle, json_str(want, "subtitle"), "{id}");
    }

    let projects = file["projects"].as_array().unwrap();
    assert_eq!(registry.projects().len(), projects.len());
    for (project, want) in registry.projects().iter().zip(projects) {
        let id = &project.id;
        assert_eq!(Some(project.id.clone()), json_str(want, "id"));
        assert_eq!(
            Some(project.folder.clone()),
            json_str(want, "folder").map(PathBuf::from),
            "{id}"
        );
        assert_eq!(project.created_at, json_time(want), "{id}");
        assert_eq!(project.removed, json_flag(want, "removed"), "{id}");
    }

    assert_eq!(
        u64::from(registry.file.version),
        file["version"].as_u64().unwrap()
    );
    assert_eq!(registry.file.active, json_str(file, "active"));
    assert_eq!(
        Some(registry.active().zns_id.clone()),
        json_str(file, "active")
    );
    assert_eq!(
        serde_json::to_value(registry.file.prefixes.to_map()).unwrap(),
        file["prefixes"]
    );
    assert_eq!(
        serde_json::to_value(registry.file.last_numbers.to_map()).unwrap(),
        file["last_numbers"]
    );
}

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
             "created_at": "2026-01-02T03:04:05Z", "project_id": "p-home", "code": "AG1"},
            {"zns_id": "0://terminal/re1", "name": "rust-engineer #1",
             "created_at": "2026-01-02T04:00:00.5Z", "definition": "rust-engineer",
             "parent": "0://terminal/main", "project_id": "p-home", "code": "RE1",
             "subtitle": "fixing the build"},
            {"zns_id": "0://terminal/re2", "name": "rust-engineer #2",
             "created_at": "2026-01-02T05:00:00Z", "definition": "rust-engineer",
             "parent": "0://terminal/re1", "project_id": "p-home", "code": "RE2"},
            {"zns_id": "0://terminal/old", "name": "Agent 2",
             "created_at": "2026-01-02T06:00:00Z", "project_id": "p-home", "archived": true,
             "code": "AG2"},
            {"zns_id": "0://terminal/docs", "name": "Agent 3",
             "created_at": "2026-01-02T07:00:00Z", "project_id": "p-home",
             "folder": away.join("docs"), "title": "Docs pass", "code": "AG3"},
            {"zns_id": "0://terminal/away", "name": "Agent",
             "created_at": "2026-01-03T00:00:00Z", "project_id": "p-away", "archived": true,
             "code": "AG4"},
        ],
        "prefixes": {"agent": "AG", "rust-engineer": "RE", "reviewer": "REV"},
        "last_numbers": {"AG": 7, "RE": 5},
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
             "created_at": "2026-01-02T03:04:05Z", "project_id": "p-home", "code": "AG1"},
            {"zns_id": "0://terminal/re1", "name": "rust-engineer #1",
             "created_at": "2026-01-02T04:00:00Z", "definition": "rust-engineer",
             "parent": "0://terminal/main", "project_id": "p-home", "code": "RE1"},
        ],
        "prefixes": {"agent": "AG", "rust-engineer": "RE"},
        "last_numbers": {"AG": 1, "RE": 1},
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

/// What a reload must keep: the list, the Archive, every project and
/// agent, and the active agent.
type State = (
    Vec<(String, usize)>,
    Vec<(String, usize)>,
    Vec<Project>,
    Vec<RegisteredAgent>,
    String,
);

fn state(registry: &AgentRegistry) -> State {
    let ids = |tree: Vec<(&RegisteredAgent, usize)>| {
        tree.into_iter()
            .map(|(a, d)| (a.agent_id().to_hex(), d))
            .collect()
    };
    (
        ids(registry.tree()),
        ids(registry.archived_tree()),
        registry.projects().to_vec(),
        registry.agents().to_vec(),
        registry.active().zns_id.clone(),
    )
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
