use super::*;

#[test]
fn instance_codes_are_numbered_like_their_names() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    for n in 1..=3 {
        let agent = registry.create_instance("rust-engineer", None).unwrap();
        assert_eq!(agent.name, format!("rust-engineer #{n}"));
        assert_eq!(code_of(&agent), format!("REN{n}"));
        let (_, name_number) = agent.name.rsplit_once('#').unwrap();
        let (_, code_number) = split_code(code_of(&agent));
        assert_eq!(code_number.to_string(), name_number);
        // What was registered is what was returned.
        assert_eq!(registry.find(agent.agent_id()).unwrap().code, agent.code);
    }
}

#[test]
fn a_one_word_definition_takes_its_first_three_letters() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let explorer = registry.create_instance("explorer", None).unwrap();
    assert_eq!(code_of(&explorer), "EXP1");
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
    // `REN` is taken: the first word's start and the second's initial.
    let research = registry.create_instance("research-engineer", None).unwrap();
    // `re`'s only candidate of its own, `REE`, is taken: `R` then `AA`.
    let re = registry.create_instance("re", None).unwrap();

    assert_eq!(code_of(&engineer), "REN1");
    assert_eq!(code_of(&reviewer), "REV1");
    assert_eq!(code_of(&second), "REV2");
    assert_eq!(code_of(&engineer_2), "REN2");
    assert_eq!(code_of(&research), "REE1");
    assert_eq!(code_of(&re), "RAA1");
    assert_codes_consistent(&registry);
}

#[test]
fn agents_without_a_definition_are_coded_age() {
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

    assert_eq!(code_of(&instance), "REN1");
    let generic: Vec<&str> = [&terminal, &unnamed, &named, &project, &in_project, &child]
        .into_iter()
        .map(code_of)
        .collect();
    assert_eq!(generic, ["AGE1", "AGE2", "AGE3", "AGE4", "AGE5", "AGE6"]);
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
    assert_eq!((code_of(&first), code_of(&second)), ("REN1", "REN2"));

    // Dropping `#1` leaves `#2`, so the next instance is `#3`: names stay
    // unique and the code still matches the name's number.
    registry.remove_project(&project).unwrap();
    let third = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(third.name, "rust-engineer #3");
    assert_eq!(code_of(&third), "REN3");

    // Dropping the highest, `#3`, does not free its number either.
    registry.forget(third.agent_id()).unwrap();
    let fourth = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(fourth.name, "rust-engineer #4");
    assert_eq!(code_of(&fourth), "REN4");
    assert_codes_consistent(&registry);
}

#[test]
fn forgetting_the_highest_instance_does_not_free_its_number() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    registry.create_instance("rust-engineer", None).unwrap();
    let second = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(code_of(&second), "REN2");

    registry.forget(second.agent_id()).unwrap();
    let next = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(next.name, "rust-engineer #3");
    assert_eq!(code_of(&next), "REN3");
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
    assert_eq!(code_of(&highest), "REN2");

    registry.remove_project(&project).unwrap();
    assert!(registry.find(highest.agent_id()).is_none());
    let next = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(next.name, "rust-engineer #3");
    assert_eq!(code_of(&next), "REN3");
    assert_codes_consistent(&registry);
}

#[test]
fn a_forgotten_generic_agents_code_is_never_handed_out_again() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let root = registry.active().clone();
    assert_eq!(code_of(&root), "AGE1");
    // As `/compare` registers a run and forgets it when it never starts.
    let run = registry.create_child("a-1", &root.zns_id, None).unwrap();
    assert_eq!(code_of(&run), "AGE2");

    registry.forget(run.agent_id()).unwrap();
    let next = registry.create_child("a-1", &root.zns_id, None).unwrap();
    assert_eq!(code_of(&next), "AGE3");
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
    assert_eq!(code_of(&next), "REN3");
    let child = reloaded.create_child("a-1", &root.zns_id, None).unwrap();
    assert_eq!(code_of(&child), "AGE3");
    assert_eq!(
        saved_last_numbers(dir.path()),
        serde_json::json!({"AGE": 3, "REN": 3})
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
            agent("0://terminal/agent", "Agent", None, Some("AGE1")),
            agent("0://terminal/re1", "rust-engineer #1", Some("rust-engineer"), Some("REN1")),
            agent("0://terminal/re2", "rust-engineer #2", Some("rust-engineer"), Some("REN2")),
            // Written before codes: backfilled, and counted.
            agent("0://terminal/re5", "rust-engineer #5", Some("rust-engineer"), None),
        ],
        "prefixes": {"agent": "AGE", "rust-engineer": "REN"},
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();

    let mut registry = open(dir.path(), launch.path());
    assert_eq!(code_of(&registry.agents()[3]), "REN5");
    assert_eq!(
        saved_last_numbers(dir.path()),
        serde_json::json!({"AGE": 1, "REN": 5})
    );
    let highest = registry.agents()[3].agent_id();
    registry.forget(highest).unwrap();
    let next = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(next.name, "rust-engineer #6");
    assert_eq!(code_of(&next), "REN6");
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
    assert_eq!(got, ["REN5", "REN3"]);
    let next = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(
        (next.name.as_str(), code_of(&next)),
        ("rust-engineer #6", "REN6")
    );
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
