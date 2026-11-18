//! The GRAPH tab's feed against real records: what a `task` spawn wrote
//! is loaded from the store, and what agents write later is added.

use super::*;
use aura_agent_kernel::RecordObserver;
use aura_core_types::SubagentDispatchRequest;
use aura_ui_terminal::events::{FlowKind, FlowLane, FlowNode};

fn task(parent: AgentId, prompt: &str) -> SubagentDispatchRequest {
    serde_json::from_value(serde_json::json!({
        "parent_agent_id": parent,
        "subagent_type": "reviewer",
        "prompt": prompt,
        "parent_permissions": aura_core_types::AgentPermissions::full_access(),
        "user_tool_defaults": aura_core_types::UserToolDefaults::default(),
    }))
    .unwrap()
}

/// Run a `task` of `parent` to its end and return the agent that did it.
async fn spawned(fixture: &Fixture, parent: AgentId, prompt: &str) -> AgentId {
    let result = fixture
        .dispatcher
        .dispatch(task(parent, prompt))
        .await
        .unwrap();
    result.child_agent_id.unwrap()
}

async fn next_flow(commands: &mut mpsc::Receiver<UiCommand>) -> UiCommand {
    tokio::time::timeout(Duration::from_secs(10), commands.recv())
        .await
        .expect("no flow was sent")
        .expect("the flow task stopped")
}

fn prompts(nodes: &[FlowNode]) -> Vec<(&str, &str)> {
    nodes
        .iter()
        .filter(|node| node.kind == FlowKind::Prompt)
        .map(|node| (node.agent_id.as_str(), node.label.as_str()))
        .collect()
}

fn parents(lanes: &[FlowLane]) -> Vec<(&str, Option<&str>)> {
    lanes
        .iter()
        .map(|lane| (lane.agent_id.as_str(), lane.parent.as_deref()))
        .collect()
}

#[tokio::test]
async fn the_flow_of_an_agent_loads_from_the_store_and_follows_new_entries() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let _state = loop_state(&fixture, &mut b);
    let (first, second) = (fixture.first, fixture.second);
    let store = Arc::clone(&fixture.hub.factory.store);
    let child = spawned(&fixture, first, "check the parser").await;

    let (feed, inbox) = FlowFeed::new();
    let (flows_tx, mut flows) = mpsc::channel(16);
    inbox.serve(&fixture.hub, flows_tx);
    feed.open(first);

    let UiCommand::FlowLoad {
        agent_id,
        lanes,
        nodes,
    } = next_flow(&mut flows).await
    else {
        panic!("expected the flow to load first");
    };
    let (first_hex, child_hex) = (first.to_hex(), child.to_hex());
    assert_eq!(agent_id, first_hex);
    assert_eq!(
        parents(&lanes),
        [
            (first_hex.as_str(), None),
            (child_hex.as_str(), Some(first_hex.as_str()))
        ]
    );
    assert_eq!(prompts(&nodes), [(child_hex.as_str(), "check the parser")]);
    assert!(nodes
        .iter()
        .any(|node| node.kind == FlowKind::Reply && node.agent_id == child_hex));

    // An agent spawned after the flow loaded joins it with its first
    // entry; agents outside the flow never do.
    let later = spawned(&fixture, first, "check the lexer").await;
    let outside = spawned(&fixture, second, "check the docs").await;
    for agent in [outside, later] {
        for entry in store.scan_record(agent, 1, 1000).unwrap() {
            feed.on_entry(&entry);
        }
    }
    let later_hex = later.to_hex();
    let mut joined = Vec::new();
    let mut added = Vec::new();
    while !added
        .iter()
        .any(|node: &FlowNode| node.kind == FlowKind::Reply)
    {
        let UiCommand::FlowUpsert {
            agent_id,
            lanes,
            nodes,
        } = next_flow(&mut flows).await
        else {
            panic!("expected an update to the flow");
        };
        assert_eq!(agent_id, first_hex);
        joined.extend(lanes);
        added.extend(nodes);
    }
    assert_eq!(
        parents(&joined),
        [(later_hex.as_str(), Some(first_hex.as_str()))]
    );
    assert_eq!(prompts(&added), [(later_hex.as_str(), "check the lexer")]);
    assert!(added.iter().all(|node| node.agent_id == later_hex));

    // Opening the flow again sends all of it, without reading the store.
    feed.open(first);
    let UiCommand::FlowLoad { lanes, nodes, .. } = next_flow(&mut flows).await else {
        panic!("expected the flow to load again");
    };
    assert_eq!(lanes.len(), 3);
    assert_eq!(
        prompts(&nodes),
        [
            (child_hex.as_str(), "check the parser"),
            (later_hex.as_str(), "check the lexer")
        ]
    );
}
