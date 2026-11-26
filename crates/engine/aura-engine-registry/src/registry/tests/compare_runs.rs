use super::*;

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
