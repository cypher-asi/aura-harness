//! A turn on a node from before the merge queue updates it first.

use super::*;
use crate::fleet::{CLAIM_COMMAND, INSTALL_BINARY};
use aura_core_protocol::cloud::{NodeClaim, NODE_PROTOCOL};
use std::sync::atomic::{AtomicBool, Ordering};

/// What a test node says it runs on `GET /v1/cloud/processes`: `None`
/// for a node from before that route (it answers 404).
type Answer = Arc<parking_lot::Mutex<Option<Vec<aura_core_protocol::cloud::MeshProcess>>>>;

/// Serve `answer` on `/v1/cloud/processes`; the node's `http://` base.
async fn node_answering(answer: Answer) -> String {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let app = axum::Router::new().route(
        "/v1/cloud/processes",
        axum::routing::get(move || {
            let answer = Arc::clone(&answer);
            async move {
                match answer.lock().clone() {
                    Some(rows) => axum::Json(rows).into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

/// A row saying a turn of `hex` runs on the node.
fn a_turn(hex: &str) -> aura_core_protocol::cloud::MeshProcess {
    aura_core_protocol::cloud::MeshProcess {
        id: "turn-1".into(),
        agent_id: hex.to_string(),
        label: "a turn started for mail".into(),
        status: "running".into(),
        started_ms: now_ms(),
        kind: None,
        run_id: Some("run-9".into()),
        parent: None,
        name: None,
    }
}

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

/// A hub with a cloud build to give, and an agent on a machine whose node
/// is from before the merge queue until that build is installed; with
/// whether it was.
async fn old_node_and_a_build() -> (
    Arc<CloudHub>,
    FakeProvider,
    tempfile::TempDir,
    AgentId,
    Arc<AtomicBool>,
    Answer,
) {
    // Idle, until a test says otherwise.
    let said: Answer = Arc::new(parking_lot::Mutex::new(Some(Vec::new())));
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
    let base = node_answering(Arc::clone(&said)).await;
    adopt_machine_at(&hub, &fake, (agent, &project, None), &base).await;
    (hub, fake, dir, agent, installed, said)
}

#[tokio::test]
async fn starting_work_on_an_old_node_puts_the_hubs_aura_on_it() {
    let (hub, fake, _dir, agent, installed, _said) = old_node_and_a_build().await;
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
async fn an_old_node_with_a_turn_no_hub_saw_end_is_updated_only_after_it() {
    let (hub, _fake, _dir, agent, installed, _said) = old_node_and_a_build().await;
    let hex = agent.to_hex();
    let mark = TurnMark {
        run_id: "run-1".into(),
        started_ms: now_ms(),
        head: 0,
        label: "Fix the build".into(),
        model: None,
        titled: false,
    };
    hub.mark_turn(agent, mark);

    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("the node the turn runs on");
    assert!(
        !installed.load(Ordering::SeqCst),
        "a restart would cut the turn off"
    );
    assert!(!hub.node_offers(&hex, "git_ship"));

    hub.end_turn_mark(agent, "run-1");
    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("a node for the next turn");
    assert!(installed.load(Ordering::SeqCst), "the next turn updates it");
    assert!(hub.node_offers(&hex, "git_ship"));
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

#[tokio::test]
async fn a_turn_the_node_started_itself_holds_off_the_update() {
    let (hub, _fake, _dir, agent, installed, said) = old_node_and_a_build().await;
    let hex = agent.to_hex();
    // No mark and no work counted here: the node started a turn for mail.
    *said.lock() = Some(vec![a_turn(&hex)]);
    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("the node the turn runs on");
    assert!(
        !installed.load(Ordering::SeqCst),
        "a restart would cut it off"
    );

    *said.lock() = Some(Vec::new());
    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("a node for the next turn");
    assert!(installed.load(Ordering::SeqCst), "idle now: updated");
}

#[tokio::test]
async fn a_node_too_old_to_say_is_updated_only_once_its_socket_said_it_is_idle() {
    let (hub, _fake, _dir, agent, installed, said) = old_node_and_a_build().await;
    let hex = agent.to_hex();
    *said.lock() = None;
    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("node");
    assert!(!installed.load(Ordering::SeqCst), "can't tell: no restart");

    hub.note_processes(&hex, vec![a_turn(&hex)]);
    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("node");
    assert!(
        !installed.load(Ordering::SeqCst),
        "its socket says a turn runs"
    );

    hub.note_processes(&hex, Vec::new());
    hub.current_node(&hex, &BootReporter::none())
        .await
        .expect("node");
    assert!(
        installed.load(Ordering::SeqCst),
        "its socket says nothing runs"
    );
}

#[tokio::test]
async fn a_node_that_cant_be_asked_is_not_updated() {
    let (hub, fake, dir) = hub();
    let binary = dir.path().join("aura-linux");
    std::fs::write(&binary, b"new").expect("write");
    let installed = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&installed);
    fake.on_exec(Arc::new(move |_, command, stdin| {
        if command == INSTALL_BINARY {
            seen.store(true, Ordering::SeqCst);
        }
        if command == CLAIM_COMMAND {
            answer(stdin, &[])
        } else {
            crate::ExecOutput::default()
        }
    }));
    let agent = AgentId::generate();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    // Nothing listens there.
    adopt_machine_at(&hub, &fake, (agent, &project, None), "http://127.0.0.1:9").await;
    hub.current_node(&agent.to_hex(), &BootReporter::none())
        .await
        .expect("the old node still runs turns");
    assert!(!installed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn two_turns_starting_at_once_update_the_node_once() {
    let (hub, fake, _dir, agent, installed, _said) = old_node_and_a_build().await;
    let hex = agent.to_hex();
    let boot = BootReporter::none();
    let (a, b) = tokio::join!(hub.current_node(&hex, &boot), hub.current_node(&hex, &boot));
    a.expect("first");
    b.expect("second");
    assert!(installed.load(Ordering::SeqCst));
    let installs = fake
        .calls()
        .iter()
        .filter(|call| call.contains(INSTALL_BINARY))
        .count();
    assert_eq!(installs, 1, "one update, the second call found it current");
}

#[tokio::test]
async fn a_cut_off_watch_keeps_the_turns_mark() {
    let (hub, _fake, _dir, agent, _installed, _said) = old_node_and_a_build().await;
    let hex = agent.to_hex();
    let mark = TurnMark {
        run_id: "run-7".into(),
        started_ms: now_ms(),
        head: 0,
        label: "Fix the build".into(),
        model: None,
        titled: false,
    };
    hub.mark_turn(agent, mark);
    // The hub stopped watching without seeing the turn end.
    hub.unsettled_run("run-7");
    hub.end_turn_mark(agent, "run-7");
    assert!(
        hub.turn_marked(&hex),
        "it may still run there: the mark stays"
    );
    // A turn seen to end clears its own mark.
    hub.end_turn_mark(agent, "run-7");
    assert!(!hub.turn_marked(&hex));
}
