//! The cloud agents the hub saved, marked while it is down: each, and the
//! agents it spawned on its machine, shows its cloud from the start, and
//! the hub coming up says the same.

use super::publish::marked_agents;
use super::view::placement_state;
use super::*;
use aura_core_auth::Owner;

/// A placed agent with a spawn on its machine, a member another user
/// placed, and a local agent.
struct Saved {
    home: tempfile::TempDir,
    placed: AgentId,
    spawn: AgentId,
    member: AgentId,
    local: AgentId,
}

fn saved() -> Saved {
    let home = tempfile::TempDir::new().unwrap();
    let (placed, spawn, member, local) = (
        AgentId::new([1; 32]),
        AgentId::new([2; 32]),
        AgentId::new([3; 32]),
        AgentId::new([4; 32]),
    );
    let row = |name: &str, machine: &str| {
        serde_json::json!({
            "zns": format!("0://{name}"),
            "name": name,
            "project": "p1",
            "project_dir": "/tmp/p1",
            "machine": machine,
        })
    };
    let mut placed_row = row("scout", "aura-sc01-0101");
    placed_row["children"] = serde_json::json!([spawn.to_hex()]);
    let mut member_row = row("reviewer", "aura-re02-0202");
    member_row["owner"] = serde_json::json!({ "kind": "zos", "user_id": "alice" });
    member_row["owner_name"] = serde_json::json!("Alice");
    let file = serde_json::json!({
        "agents": { placed.to_hex(): placed_row, member.to_hex(): member_row },
        "install": "test1",
    });
    std::fs::write(home.path().join("placements.json"), file.to_string()).unwrap();
    Saved {
        home,
        placed,
        spawn,
        member,
        local,
    }
}

/// Where `agent` runs as the badge reads it: placed in the cloud, and by
/// whom when that is another user.
fn badge(state: &PlacementState) -> (PlacementView, Option<String>) {
    (state.placement, state.owner.clone())
}

#[test]
fn with_the_hub_down_saved_cloud_agents_and_their_spawns_are_marked_cloud() {
    let s = saved();
    let shared = Shared::default();
    *shared.saved.lock() = aura_engine_cloud::hub::saved_placements(s.home.path());
    let marked = marked_agents(None, &shared, Some(s.local));
    for agent in [s.placed, s.spawn, s.member, s.local] {
        assert!(marked.contains(&agent), "{agent} is sent its placement");
    }

    let me = Owner::Local;
    let state = |agent| badge(&placement_state(None, &shared, agent, &me));
    assert_eq!(state(s.placed), (PlacementView::Cloud, None));
    assert_eq!(
        state(s.spawn),
        (PlacementView::Cloud, None),
        "on its placer's machine"
    );
    assert_eq!(
        state(s.member),
        (PlacementView::Cloud, Some("Alice".to_string())),
        "another user's, named"
    );
    assert_eq!(
        state(s.local),
        (PlacementView::Local, None),
        "not saved: local"
    );
}

#[test]
fn without_saved_placements_nothing_but_the_agent_on_screen_is_marked() {
    let shared = Shared::default();
    let agent = AgentId::new([9; 32]);
    assert_eq!(marked_agents(None, &shared, Some(agent)), [agent]);
    assert!(marked_agents(None, &shared, None).is_empty());
    let state = placement_state(None, &shared, agent, &Owner::Local);
    assert_eq!(state.placement, PlacementView::Local);
}

#[test]
fn the_hub_coming_up_marks_the_saved_agents_the_same() {
    let s = saved();
    let shared = Shared::default();
    *shared.saved.lock() = aura_engine_cloud::hub::saved_placements(s.home.path());
    let me = Owner::Local;
    let down: Vec<_> = [s.placed, s.spawn, s.member, s.local]
        .into_iter()
        .map(|agent| badge(&placement_state(None, &shared, agent, &me)))
        .collect();

    let db = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn Store> =
        Arc::new(aura_store_db::RocksStore::open(db.path(), false).unwrap());
    let hub = CloudHub::new(
        aura_config::CloudConfig::defaults(),
        store,
        s.home.path().to_path_buf(),
        Arc::new(aura_engine_cloud::FakeProvider::new()),
        aura_engine_cloud::snapshots::AuraBinary::Installed,
    )
    .unwrap();
    let up: Vec<_> = [s.placed, s.spawn, s.member, s.local]
        .into_iter()
        .map(|agent| badge(&placement_state(Some(&hub), &shared, agent, &me)))
        .collect();
    assert_eq!(up, down, "the cloud stays where it was");
    let marked = marked_agents(Some(&hub), &shared, None);
    for agent in [s.placed, s.spawn, s.member] {
        assert!(
            marked.contains(&agent),
            "{agent} still marked with the hub up"
        );
    }
}
