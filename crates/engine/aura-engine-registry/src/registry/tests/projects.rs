use super::*;

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
    registry.archive_at(first.agent_id(), at(1)).unwrap();
    registry.archive_at(second.agent_id(), at(2)).unwrap();
    registry.archive_at(third.agent_id(), at(3)).unwrap();
    assert_eq!(names(registry.tree()), [(legacy.name.clone(), 0)]);

    registry.restore(second.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [(legacy.name.clone(), 0), (second.name.clone(), 0)]
    );
    // The Archive lists the newest archived first.
    assert_eq!(
        names(registry.archived_tree()),
        [(third.name.clone(), 0), (first.name.clone(), 0)]
    );

    // It comes back in its old place.
    registry.restore(first.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [(legacy.name, 0), (first.name, 0), (second.name, 0)]
    );
}

/// A project in its own folder with two agents besides its first, an
/// instance under each, the first added agent archived at 1 and the
/// second's instance at 2.
struct ArchivedInProject {
    dir: tempfile::TempDir,
    launch: tempfile::TempDir,
    folder: tempfile::TempDir,
    registry: AgentRegistry,
    /// The launch project's agent, active.
    legacy: RegisteredAgent,
    /// The project's first agent.
    site: RegisteredAgent,
    project: String,
    added: RegisteredAgent,
    other: RegisteredAgent,
    spawned: RegisteredAgent,
    nested: RegisteredAgent,
}

fn archived_in_project() -> ArchivedInProject {
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
    registry.archive_at(added.agent_id(), at(1)).unwrap();
    registry.archive_at(nested.agent_id(), at(2)).unwrap();
    ArchivedInProject {
        dir,
        launch,
        folder,
        registry,
        legacy,
        site,
        project,
        added,
        other,
        spawned,
        nested,
    }
}

#[test]
fn restoring_an_agent_whose_project_was_removed_puts_back_just_that_agent() {
    let f = archived_in_project();
    let mut registry = f.registry;

    let gone = registry.remove_project(&f.project).unwrap();
    assert_eq!(gone, [f.site.agent_id(), f.other.agent_id()]);
    assert!(registry.find(f.site.agent_id()).is_none());
    assert!(registry.find(f.other.agent_id()).unwrap().removed);
    assert_eq!(names(registry.tree()), [(f.legacy.name.clone(), 0)]);
    // Newest archived first; what an agent spawned stays under it.
    assert_eq!(
        names(registry.archived_tree()),
        [
            (f.nested.name.clone(), 0),
            (f.added.name.clone(), 0),
            (f.spawned.name.clone(), 1)
        ]
    );

    // Survives a reload, then comes back in the same project, alone.
    let mut registry = open(f.dir.path(), f.launch.path());
    assert_eq!(registry.active(), &f.legacy);
    registry.restore(f.added.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [
            (f.legacy.name.clone(), 0),
            (f.added.name.clone(), 0),
            (f.spawned.name.clone(), 1)
        ]
    );
    assert_eq!(
        registry.project_of(f.added.agent_id()).unwrap().id,
        f.project
    );
    assert_eq!(
        registry.project_folder(f.spawned.agent_id()),
        Some(f.folder.path().canonicalize().unwrap())
    );
    assert_eq!(
        names(registry.archived_tree()),
        [(f.nested.name.clone(), 0)]
    );

    // A nested agent brings back the agent that spawned it.
    registry.restore(f.nested.agent_id()).unwrap();
    assert_eq!(
        names(registry.tree()),
        [
            (f.legacy.name.clone(), 0),
            (f.added.name.clone(), 0),
            (f.spawned.name.clone(), 1),
            (f.other.name.clone(), 0),
            (f.nested.name.clone(), 1)
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
