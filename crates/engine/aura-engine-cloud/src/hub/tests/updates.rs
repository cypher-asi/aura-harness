//! A turn on a node from before the merge queue updates it first.

use super::*;
use crate::fleet::{CLAIM_COMMAND, INSTALL_BINARY};
use aura_core_protocol::cloud::{NodeClaim, NODE_PROTOCOL};
use std::sync::atomic::{AtomicBool, Ordering};

/// What the machine's node answers a claim with: `tools` empty for one
/// from before the merge queue.
fn answer(stdin: Option<&[u8]>, tools: &[&str]) -> crate::ExecOutput {
    let claim: NodeClaim = serde_json::from_slice(stdin.unwrap_or_default()).expect("claim");
    let answer = serde_json::json!({
        "node_id": claim.node_id,
        "port": 8080,
        "sealed": 0,
        "protocol": NODE_PROTOCOL,
        "tools": tools,
    });
    crate::ExecOutput {
        exit_code: 0,
        stdout: serde_json::to_vec(&answer).expect("json"),
        stderr: Vec::new(),
    }
}

#[tokio::test]
async fn starting_work_on_an_old_node_puts_the_hubs_aura_on_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let binary = dir.path().join("aura-linux");
    std::fs::write(&binary, b"new aura build").expect("write");
    let store: Arc<dyn Store> =
        Arc::new(RocksStore::open(dir.path().join("db"), false).expect("open"));
    let fake = FakeProvider::new();
    let hub = CloudHub::new(
        CloudConfig::defaults(),
        store,
        dir.path().join("cloud"),
        Arc::new(fake.clone()),
        AuraBinary::Upload(binary),
    )
    .expect("hub");
    let installed = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&installed);
    fake.on_exec(Arc::new(move |_, command, stdin| {
        if command == INSTALL_BINARY {
            seen.store(true, Ordering::SeqCst);
        }
        if command != CLAIM_COMMAND {
            return crate::ExecOutput::default();
        }
        if seen.load(Ordering::SeqCst) {
            answer(
                stdin,
                &["git_commit", "git_push", "git_commit_push", "git_ship"],
            )
        } else {
            answer(stdin, &[])
        }
    }));
    let agent = AgentId::generate();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    adopt_machine(&hub, &fake, agent, &project, None).await;
    let hex = agent.to_hex();

    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("a node to run the turn on");
    assert!(installed.load(Ordering::SeqCst), "the hub's build went on");
    assert!(hub.fleet.node_offers(&hex, "git_ship"));
    assert_eq!(
        fake.file("aura-sc01-0101", "/tmp/aura.update").as_deref(),
        Some(&b"new aura build"[..])
    );
}

#[tokio::test]
async fn without_a_build_to_give_an_old_node_keeps_working_without_git_ship() {
    let (hub, fake, dir) = hub();
    fake.on_exec(Arc::new(|_, command, stdin| {
        assert_ne!(command, INSTALL_BINARY, "nothing to install");
        if command == CLAIM_COMMAND {
            answer(stdin, &[])
        } else {
            crate::ExecOutput::default()
        }
    }));
    let agent = AgentId::generate();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    adopt_machine(&hub, &fake, agent, &project, None).await;
    let hex = agent.to_hex();
    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("the old node still runs turns");
    assert!(
        !hub.node_offers(&hex, "git_ship"),
        "so it gets the fallback prompt"
    );
}
