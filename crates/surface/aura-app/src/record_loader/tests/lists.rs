use super::*;

#[test]
fn last_activity_is_reused_until_the_head_moves() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let mut cache = LastActive::default();
    assert_eq!(cache.of(&store, agent), (0, None));

    let mut first = text_tx(agent, TransactionType::AgentMsg, "one");
    first.ts_ms = 1_000;
    append(&store, 1, first);
    assert_eq!(cache.of(&store, agent), (1, Some(1_000)));
    // Same head: answered from the cache, not the entry.
    cache.0.insert(agent, (1, Some(7)));
    assert_eq!(cache.of(&store, agent), (1, Some(7)));

    let mut second = text_tx(agent, TransactionType::AgentMsg, "two");
    second.ts_ms = 2_000;
    append(&store, 2, second);
    assert_eq!(cache.of(&store, agent), (2, Some(2_000)));
}

#[test]
fn a_shown_agents_reads_are_kept_until_its_record_grows() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let mut kept = ByHead::default();
    let reads = std::cell::Cell::new(0);
    let read = |kept: &mut ByHead<usize>| {
        kept.get_or_read(&store, agent, || {
            reads.set(reads.get() + 1);
            Ok(reads.get())
        })
        .unwrap()
    };
    assert_eq!(read(&mut kept), 1);
    assert_eq!(read(&mut kept), 1, "same head, nothing read");

    append(&store, 1, text_tx(agent, TransactionType::AgentMsg, "one"));
    assert_eq!(read(&mut kept), 2, "the record grew");
    assert_eq!(read(&mut kept), 2);
}

#[test]
fn list_rows_match_the_registry_for_compares_and_the_archive() {
    use crate::agent_registry::AgentRegistry;
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store_dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(store_dir.path(), false).unwrap();
    let mut registry = AgentRegistry::load_or_init_in(data.path(), folder.path()).unwrap();
    let lead = registry.active().clone();
    let compare = registry.create_compare(&lead.zns_id).unwrap();
    let arm = registry
        .create_compare_arm("gate", &compare.zns_id)
        .unwrap();
    let run = registry.create_child("gate-2", &arm.zns_id, None).unwrap();
    let run_b = registry.create_child("gate-1", &arm.zns_id, None).unwrap();
    let sub = registry.create_child("Sub", &run.zns_id, None).unwrap();
    let archived_run = registry.create_child("gate-3", &arm.zns_id, None).unwrap();
    let (elsewhere, _) = registry.open_project(other.path()).unwrap();
    let spawn = registry
        .create_child("Spawn", &elsewhere.zns_id, None)
        .unwrap();
    registry.archive(archived_run.agent_id()).unwrap();
    registry.archive(spawn.agent_id()).unwrap();
    append(
        &store,
        1,
        text_tx(sub.agent_id(), TransactionType::AgentMsg, "hi"),
    );

    let running = HashSet::from([run_b.agent_id()]);
    let active = registry.active().agent_id();
    let mut rows = ListRows::build(&registry, &running, active);
    rows.read_activity(&store, &mut LastActive::default());
    assert_eq!(rows.active, active);
    check_rows(&registry, &store, &rows.listed, registry.tree(), active);
    check_rows(
        &registry,
        &store,
        &rows.archived,
        registry.archived_tree(),
        AgentId::new([0; 32]),
    );
    assert!(rows
        .listed
        .iter()
        .any(|(id, r)| *id == run_b.agent_id() && r.running));
    assert!(rows
        .archived
        .iter()
        .any(|(id, r)| *id == archived_run.agent_id() && r.bench_run));
    check_projects(&rows, &registry);
}

/// Assert the project rows match the registry's projects and counts.
fn check_projects(rows: &ListRows, registry: &crate::agent_registry::AgentRegistry) {
    assert_eq!(rows.projects, project_summaries(registry));
    for project in &rows.projects {
        assert_eq!(
            (project.listed_agents, project.archived_agents),
            registry.project_counts(&project.id)
        );
    }
}

#[test]
fn hiding_every_listed_row_of_a_project_keeps_it_on_the_projects_list() {
    let (only_hidden, member, visible) = (
        AgentId::new([1; 32]),
        AgentId::new([2; 32]),
        AgentId::new([3; 32]),
    );
    let row = |id: AgentId, project: &str| {
        (
            id,
            AgentSummary {
                id: id.to_hex(),
                project_id: Some(project.to_string()),
                ..AgentSummary::default()
            },
        )
    };
    let project = |id: &str, listed_agents: usize| ProjectSummary {
        id: id.to_string(),
        listed_agents,
        ..ProjectSummary::default()
    };
    let mut rows = ListRows {
        projects: vec![project("launch", 1), project("shared", 2)],
        listed: vec![
            row(only_hidden, "launch"),
            row(member, "shared"),
            row(visible, "shared"),
        ],
        archived: Vec::new(),
        members: Vec::new(),
        active: visible,
    };
    rows.split_members(
        &HashSet::from([only_hidden, member]),
        &HashSet::from([only_hidden]),
    );

    let listed: Vec<AgentId> = rows.listed.iter().map(|(id, _)| *id).collect();
    assert_eq!(listed, [member, visible]);
    let members: Vec<AgentId> = rows.members.iter().map(|(id, _)| *id).collect();
    assert_eq!(members, [only_hidden, member]);
    // The launch project, holding only a hidden member, is still listed
    // (a channel can still be given it), counting no agent.
    let counts: Vec<(&str, usize)> = rows
        .projects
        .iter()
        .map(|p| (p.id.as_str(), p.listed_agents))
        .collect();
    assert_eq!(counts, [("launch", 0), ("shared", 2)]);
}
