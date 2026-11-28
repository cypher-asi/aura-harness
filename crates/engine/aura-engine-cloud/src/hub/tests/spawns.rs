//! The agents placed agents spawned on their machines.

use super::*;
use aura_core_protocol::cloud::{MeshProcess, MESH_PROCESS_AGENT};

/// A placement whose machine spawned `children`, copied with it.
fn placed_with(children: &[AgentId]) -> PlacedAgent {
    PlacedAgent {
        zns: "0://scout".into(),
        name: "scout".into(),
        project: "p1".into(),
        project_dir: std::path::PathBuf::from("/p1"),
        machine: "aura-sc01-0101".into(),
        owner: None,
        owner_name: None,
        branch: None,
        repo: None,
        fetched_tip: None,
        children: children.iter().map(AgentId::to_hex).collect(),
        turn: None,
        slept_ms: None,
    }
}

#[test]
fn spawned_agents_lead_to_the_agent_placed_on_their_machine() {
    let (hub, _fake, _dir) = hub();
    let (root, other) = (AgentId::new([1; 32]), AgentId::new([2; 32]));
    let (child, grandchild) = (AgentId::new([3; 32]), AgentId::new([4; 32]));
    {
        let mut placements = hub.placements.lock();
        placements
            .agents
            .insert(root.to_hex(), placed_with(&[child, grandchild]));
        placements.agents.insert(other.to_hex(), placed_with(&[]));
    }

    let mut spawns = hub.machine_spawns();
    spawns.sort_by_key(|(_, spawned)| spawned.to_hex());
    assert_eq!(spawns, [(root, child), (root, grandchild)]);
    assert_eq!(hub.machine_root(child), Some(root));
    assert_eq!(
        hub.machine_root(grandchild),
        Some(root),
        "copied with the agent placed there, at any depth"
    );
    assert_eq!(hub.machine_root(root), None, "placed, not spawned there");
    assert_eq!(hub.machine_root(AgentId::new([5; 32])), None);
}

#[test]
fn spawned_agents_work_while_their_machine_says_they_run() {
    let (hub, _fake, _dir) = hub();
    let root = AgentId::new([1; 32]);
    let (busy, idle) = (AgentId::new([3; 32]), AgentId::new([4; 32]));
    hub.placements
        .lock()
        .agents
        .insert(root.to_hex(), placed_with(&[busy, idle]));
    let row = |agent: AgentId, status: &str, spawned: bool| MeshProcess {
        id: format!("row:{}", agent.to_hex()),
        agent_id: agent.to_hex(),
        label: "explore".into(),
        status: status.into(),
        started_ms: 1,
        kind: spawned.then(|| MESH_PROCESS_AGENT.into()),
        run_id: None,
        parent: spawned.then(|| root.to_hex()),
        name: spawned.then(|| "explore".into()),
    };

    hub.note_processes(
        &root.to_hex(),
        vec![
            row(root, "running", false),
            row(busy, "running", true),
            row(idle, "idle", true),
        ],
    );

    assert_eq!(
        hub.working_spawns(),
        std::collections::HashSet::from([busy]),
        "the placed agent's own turn is not a spawn's"
    );
}
