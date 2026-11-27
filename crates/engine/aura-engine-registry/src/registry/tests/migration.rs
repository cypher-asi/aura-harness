use super::*;

#[test]
fn a_version_0_registry_becomes_one_project_per_folder() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let site = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    write_version_0_registry(dir.path(), site.path(), docs.path());

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
    assert_version_0_lists(&registry);

    let body = stored_body(dir.path());
    assert!(!body.contains("member_of"), "{body}");
    assert!(body.contains("\"version\": 4"), "{body}");
}

/// Store `file` in `dir`'s database as an older release left it, without
/// bringing it up to date.
fn write_store(dir: &Path, file: &RegistryFile) {
    let rocks = shared_store(&dir.join(DB_DIR)).unwrap();
    let (diff, _) = persist::Persisted::default().diff(persist::to_rows(file));
    AgentStore::for_store(&rocks).apply(&diff).unwrap();
}

/// A version 3 registry with a channel coordinator named the old way,
/// and agents whose names look like it but that are not coordinators or
/// were named otherwise.
fn version_3_with_coordinators(launch: &Path) -> RegistryFile {
    let coordinator = Some(crate::definitions::COORDINATOR_DEFINITION);
    let agent = |zns: &str, name: &str, definition: Option<&str>, code: &str| RegisteredAgent {
        zns_id: zns.to_string(),
        definition: definition.map(str::to_string),
        project_id: Some("p".to_string()),
        code: Some(code.to_string()),
        ..AgentRegistry::new_agent(name.to_string())
    };
    RegistryFile {
        version: 3,
        projects: vec![Project {
            id: "p".to_string(),
            folder: launch.canonicalize().unwrap(),
            created_at: at(0),
            removed: false,
        }],
        agents: vec![
            agent("0://terminal/main", "Terminal Agent", None, "AGE1"),
            agent(
                "0://terminal/site",
                "#site coordinator",
                coordinator,
                "COO1",
            ),
            agent("0://terminal/plain", "#site coordinator", None, "AGE2"),
            agent(
                "0://terminal/rev",
                "#ops coordinator",
                Some("reviewer"),
                "REV1",
            ),
            agent("0://terminal/mine", "my coordinator", coordinator, "COO2"),
        ],
        ..RegistryFile::default()
    }
}

#[test]
fn a_version_3_store_renames_channel_coordinators_and_keeps_codes() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    write_store(dir.path(), &version_3_with_coordinators(launch.path()));

    let expected = [
        ("Terminal Agent", "AGE1"),
        ("coordinator", "COO1"),
        ("#site coordinator", "AGE2"),
        ("#ops coordinator", "REV1"),
        ("my coordinator", "COO2"),
    ]
    .map(|(name, code)| (name.to_string(), code.to_string()));
    let named = |registry: &AgentRegistry| -> Vec<(String, String)> {
        registry
            .agents()
            .iter()
            .map(|a| (a.name.clone(), code_of(a).to_string()))
            .collect()
    };
    let registry = open(dir.path(), launch.path());
    assert_eq!(named(&registry), expected);
    assert_eq!(registry.file.version, REGISTRY_VERSION);
    drop(registry);

    // Saved renamed: the store's row has the new name, and loading again
    // changes nothing.
    let body = stored_body(dir.path());
    assert!(body.contains("\"version\": 4"), "{body}");
    let rocks = shared_store(&dir.path().join(DB_DIR)).unwrap();
    let rows = AgentStore::for_store(&rocks).load().unwrap().agents;
    drop(rocks);
    let site = rows
        .iter()
        .find(|r| r.zns_id == "0://terminal/site")
        .unwrap();
    assert_eq!(site.name, "coordinator");
    assert_eq!(site.code.as_deref(), Some("COO1"));
    assert_eq!(named(&open(dir.path(), launch.path())), expected);
    assert_eq!(stored_body(dir.path()), body);
}

/// Write a version 0 registry, with projects in `site` and `docs`, and the
/// desktop app's tabs, into `dir`.
fn write_version_0_registry(dir: &Path, site: &Path, docs: &Path) {
    let site_path = site.display().to_string();
    let docs_path = docs.display().to_string();
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
    fs::write(dir.join(REGISTRY_FILE), body).unwrap();
    // The desktop app had the site project before the terminal's.
    fs::write(
        dir.join(DESKTOP_TABS_FILE),
        format!(
            "{}\n{}\n",
            hex("0://terminal/site"),
            hex("0://terminal/term")
        ),
    )
    .unwrap();
}

/// What the version 0 registry of [`write_version_0_registry`] lists after
/// its migration, and what it keeps in the Archive.
fn assert_version_0_lists(registry: &AgentRegistry) {
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
