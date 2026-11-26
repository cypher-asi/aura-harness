//! Agent status, processing claims and runtime capabilities.

use super::support::*;
use crate::store::ReadStore;
use crate::store::WriteStore;
use aura_core_types::{AgentId, AgentStatus, RecordEntry};
use aura_core_types::{
    InstalledIntegrationDefinition, InstalledToolCapability, InstalledToolIntegrationRequirement,
    RuntimeCapabilityInstall, SystemKind,
};
use std::collections::HashMap;

#[test]
fn test_agent_status() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let status = store.get_agent_status(agent_id).unwrap();
    assert_eq!(status, AgentStatus::Active);

    store
        .set_agent_status(agent_id, AgentStatus::Paused)
        .unwrap();
    let status = store.get_agent_status(agent_id).unwrap();
    assert_eq!(status, AgentStatus::Paused);
}

#[test]
fn test_processing_claim_lifecycle() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    assert!(!store.is_agent_processing(agent_id).unwrap());
    assert!(store.try_claim_agent_processing(agent_id).unwrap());
    assert!(store.is_agent_processing(agent_id).unwrap());
    assert!(!store.try_claim_agent_processing(agent_id).unwrap());

    store.release_agent_processing(agent_id).unwrap();
    assert!(!store.is_agent_processing(agent_id).unwrap());
    assert!(store.try_claim_agent_processing(agent_id).unwrap());
    store.release_agent_processing(agent_id).unwrap();
}

#[test]
fn test_empty_agent_state() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 0);
    assert!(!store.has_pending_tx(agent_id).unwrap());
    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 0);
    assert!(store.dequeue_tx(agent_id).unwrap().is_none());
}

#[test]
fn test_multiple_agents_isolated() {
    let (store, _dir) = create_test_store();

    let agent1 = AgentId::generate();
    let agent2 = AgentId::generate();

    let tx1 = create_test_tx(agent1);
    let tx2 = create_test_tx(agent2);

    store.enqueue_tx(&tx1).unwrap();
    store.enqueue_tx(&tx2).unwrap();

    assert_eq!(store.get_inbox_depth(agent1).unwrap(), 1);
    assert_eq!(store.get_inbox_depth(agent2).unwrap(), 1);

    let (token, tx) = store.dequeue_tx(agent1).unwrap().unwrap();
    let entry = RecordEntry::builder(1, tx).build();
    store
        .append_entry_atomic(agent1, 1, &entry, token.inbox_seq())
        .unwrap();

    assert_eq!(store.get_head_seq(agent1).unwrap(), 1);
    assert_eq!(store.get_head_seq(agent2).unwrap(), 0);
}

#[test]
fn test_agent_status_transitions() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    assert_eq!(
        store.get_agent_status(agent_id).unwrap(),
        AgentStatus::Active
    );

    store
        .set_agent_status(agent_id, AgentStatus::Paused)
        .unwrap();
    assert_eq!(
        store.get_agent_status(agent_id).unwrap(),
        AgentStatus::Paused
    );

    store.set_agent_status(agent_id, AgentStatus::Dead).unwrap();
    assert_eq!(store.get_agent_status(agent_id).unwrap(), AgentStatus::Dead);

    store
        .set_agent_status(agent_id, AgentStatus::Active)
        .unwrap();
    assert_eq!(
        store.get_agent_status(agent_id).unwrap(),
        AgentStatus::Active
    );
}

#[test]
fn test_runtime_capabilities_round_trip() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(1, tx).build();
    let runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![InstalledIntegrationDefinition {
            integration_id: "integration-brave-1".to_string(),
            name: "Brave Search".to_string(),
            provider: "brave_search".to_string(),
            kind: "workspace_integration".to_string(),
            metadata: HashMap::new(),
        }],
        installed_tools: vec![InstalledToolCapability {
            name: "brave_search_web".to_string(),
            required_integration: Some(InstalledToolIntegrationRequirement {
                integration_id: None,
                provider: Some("brave_search".to_string()),
                kind: Some("workspace_integration".to_string()),
            }),
        }],
    };

    store
        .append_entry_direct_with_runtime_capabilities(
            agent_id,
            1,
            &entry,
            Some(&runtime_capabilities),
            false,
        )
        .unwrap();

    let persisted = store.get_runtime_capabilities(agent_id).unwrap();
    assert_eq!(persisted, Some(runtime_capabilities));
}

#[test]
fn test_runtime_capabilities_can_be_cleared_atomically() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let entry1 = RecordEntry::builder(1, create_test_tx(agent_id)).build();
    let runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![],
        installed_tools: vec![InstalledToolCapability {
            name: "brave_search_web".to_string(),
            required_integration: None,
        }],
    };
    store
        .append_entry_direct_with_runtime_capabilities(
            agent_id,
            1,
            &entry1,
            Some(&runtime_capabilities),
            false,
        )
        .unwrap();
    assert!(store.get_runtime_capabilities(agent_id).unwrap().is_some());

    let entry2 = RecordEntry::builder(2, create_test_tx(agent_id)).build();
    store
        .append_entry_direct_with_runtime_capabilities(agent_id, 2, &entry2, None, true)
        .unwrap();

    assert_eq!(store.get_runtime_capabilities(agent_id).unwrap(), None);
}
