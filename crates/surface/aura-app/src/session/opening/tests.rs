use super::*;
use aura_store_db::agents::derive_agent_id;
use aura_store_db::{AgentRecord, AgentStore, NewChannel};

/// A registry with three agents in a fresh store, and its store.
fn registry() -> (tempfile::TempDir, RocksStore, AgentRegistry, Vec<AgentId>) {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path().join("db"), false).unwrap();
    let mut registry =
        AgentRegistry::open(AgentStore::for_store(&store), dir.path(), dir.path()).unwrap();
    let mut ids = vec![registry.active().agent_id()];
    for _ in 0..2 {
        ids.push(registry.create(None).unwrap().agent_id());
    }
    registry.set_active(ids[0]).unwrap();
    (dir, store, registry, ids)
}

#[test]
fn it_opens_on_the_agent_the_ui_was_last_in() {
    let (_dir, store, mut registry, ids) = registry();
    let open_on = OpenOn::Agent(ids[2].to_hex());
    let opened = agent_to_open(&mut registry, &store, Some(&open_on));
    assert_eq!(opened.agent_id(), ids[2]);
    assert_eq!(registry.active().agent_id(), ids[2], "and makes it active");
}

#[test]
fn an_unknown_or_archived_agent_opens_the_active_one() {
    let (_dir, store, mut registry, ids) = registry();
    let unknown = OpenOn::Agent(AgentId::generate().to_hex());
    assert_eq!(
        agent_to_open(&mut registry, &store, Some(&unknown)).agent_id(),
        ids[0]
    );
    let bad = OpenOn::Agent("not hex".to_string());
    assert_eq!(
        agent_to_open(&mut registry, &store, Some(&bad)).agent_id(),
        ids[0]
    );
    registry.archive(ids[1]).unwrap();
    let archived = OpenOn::Agent(ids[1].to_hex());
    assert_eq!(
        agent_to_open(&mut registry, &store, Some(&archived)).agent_id(),
        ids[0]
    );
    assert_eq!(
        agent_to_open(&mut registry, &store, None).agent_id(),
        ids[0]
    );
}

#[test]
fn it_opens_on_the_coordinator_of_the_channel_the_ui_was_last_in() {
    let (dir, store, _registry, ids) = registry();
    let coordinator = AgentRecord {
        agent_id: derive_agent_id("zns-coordinator"),
        zns_id: "zns-coordinator".to_string(),
        name: "coordinator".to_string(),
        created_at: chrono::Utc::now(),
        ..AgentStore::for_store(&store).load().unwrap().agents[0].clone()
    };
    let new = NewChannel {
        slug: "zode".to_string(),
        title: "Zode".to_string(),
        ..NewChannel::default()
    };
    let channel = ChannelStore::for_store(&store)
        .create(new, &coordinator, chrono::Utc::now())
        .unwrap();
    // The channel's coordinator joined the registry's store with it.
    let mut registry =
        AgentRegistry::open(AgentStore::for_store(&store), dir.path(), dir.path()).unwrap();
    let open_on = OpenOn::Channel(channel.id.clone());
    let opened = agent_to_open(&mut registry, &store, Some(&open_on));
    assert_eq!(opened.agent_id(), channel.coordinator);
    let gone = OpenOn::Channel("no-such-channel".to_string());
    registry.set_active(ids[0]).unwrap();
    assert_eq!(
        agent_to_open(&mut registry, &store, Some(&gone)).agent_id(),
        ids[0]
    );
}
