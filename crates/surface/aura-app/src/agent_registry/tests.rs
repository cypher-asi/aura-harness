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
    assert!(dir.path().join(REGISTRY_FILE).exists());
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
    registry.save().unwrap();

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

    let body = fs::read_to_string(dir.path().join(REGISTRY_FILE)).unwrap();
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
