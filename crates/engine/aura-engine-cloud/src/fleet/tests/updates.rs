//! Bringing a node from before the merge queue up to the hub's aura.

use super::*;
use crate::fleet::update::STAGED_BINARY;
use crate::fleet::{NodeUpdate, INSTALL_BINARY};
use crate::snapshots::STOP_NODE;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a node from before the merge queue answers a claim with: the
/// protocol of the day, and no tools.
fn claimed_before_the_queue(claim: &NodeClaim) -> crate::ExecOutput {
    let answer = serde_json::json!({
        "node_id": claim.node_id,
        "port": 8080,
        "sealed": claim.seal.len(),
        "protocol": NODE_PROTOCOL,
    });
    crate::ExecOutput {
        exit_code: 0,
        stdout: serde_json::to_vec(&answer).expect("json"),
        stderr: Vec::new(),
    }
}

/// A fleet whose machine `aa` runs a node from before the merge queue
/// until the hub installs a new aura, then a current one. With
/// `install_fails`, installing fails. Records every command run.
async fn old_node(install_fails: bool) -> (Arc<VmFleet>, FakeProvider, Arc<Mutex<Vec<String>>>) {
    let (fleet, fake, _) = fleet(Duration::from_secs(300));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let updated = Arc::new(AtomicBool::new(false));
    let (seen, now_current) = (Arc::clone(&commands), Arc::clone(&updated));
    fake.on_exec(Arc::new(move |_, command, stdin| {
        seen.lock().push(command.to_string());
        if command == INSTALL_BINARY {
            if install_fails {
                return crate::ExecOutput {
                    exit_code: 1,
                    stdout: Vec::new(),
                    stderr: b"cannot execute binary file".to_vec(),
                };
            }
            now_current.store(true, Ordering::SeqCst);
        }
        if command != CLAIM_COMMAND {
            return crate::ExecOutput::default();
        }
        let claim: NodeClaim =
            serde_json::from_slice(stdin.unwrap_or_default()).expect("claim json");
        if now_current.load(Ordering::SeqCst) {
            claimed(&claim, NODE_PROTOCOL)
        } else {
            claimed_before_the_queue(&claim)
        }
    }));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("provision");
    (fleet, fake, commands)
}

#[tokio::test]
async fn an_old_node_gets_the_hubs_aura_and_offers_git_ship_after() {
    let (fleet, fake, commands) = old_node(false).await;
    assert!(
        !fleet.node_offers("aa", "git_ship"),
        "a node from before the queue"
    );
    let update = fleet
        .update_node("aa", b"new aura".to_vec(), &BootReporter::none())
        .await
        .expect("update");
    assert_eq!(update, NodeUpdate::Updated);
    assert_eq!(
        fake.file("aura-re36-aa", STAGED_BINARY).as_deref(),
        Some(&b"new aura"[..])
    );
    let ran = commands.lock().clone();
    let install = ran
        .iter()
        .position(|c| c == INSTALL_BINARY)
        .expect("installed");
    let restart = ran
        .iter()
        .position(|c| c.starts_with(STOP_NODE))
        .expect("restarted");
    let claim = ran
        .iter()
        .rposition(|c| c == CLAIM_COMMAND)
        .expect("claimed");
    assert!(install < restart && restart < claim, "{ran:?}");
    assert!(fleet.node_offers("aa", "git_ship"));

    // Current now: nothing more is done.
    let again = fleet
        .update_node("aa", b"new aura".to_vec(), &BootReporter::none())
        .await
        .expect("update");
    assert_eq!(again, NodeUpdate::Current);
}

#[tokio::test]
async fn a_machine_at_work_is_never_updated() {
    let (fleet, _fake, commands) = old_node(false).await;
    let work = fleet.begin_work("aa");
    let update = fleet
        .update_node("aa", b"new aura".to_vec(), &BootReporter::none())
        .await
        .expect("update");
    assert_eq!(update, NodeUpdate::Busy);
    assert!(!commands.lock().iter().any(|c| c == INSTALL_BINARY));
    drop(work);
}

#[tokio::test]
async fn a_failed_install_keeps_the_old_node_running() {
    let (fleet, _fake, commands) = old_node(true).await;
    let err = fleet
        .update_node("aa", b"broken".to_vec(), &BootReporter::none())
        .await
        .expect_err("install fails");
    assert!(err.to_string().contains("install"), "{err}");
    assert!(
        !commands.lock().iter().any(|c| c.starts_with(STOP_NODE)),
        "the old node was never stopped"
    );
    assert!(!fleet.node_offers("aa", "git_ship"));
}
