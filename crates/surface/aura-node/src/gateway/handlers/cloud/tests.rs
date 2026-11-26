use super::*;
use crate::config::NodeConfig;
use crate::gateway::{create_router, RouterStateConfig};
use aura_core_protocol::cloud::NodeClaim;
use aura_core_types::{RecordEntry, Transaction};
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::{MockProvider, ModelProvider};
use aura_store_db::{RocksStore, Store};
use axum::body::Body;
use axum::http::Request;
use tower::util::ServiceExt;

const NODE_TOKEN: &str = "node-token-for-tests-0123";

fn cloud_state(dir: &std::path::Path) -> RouterState {
    cloud_state_in(dir, None)
}

fn cloud_state_in(dir: &std::path::Path, workspace: Option<std::path::PathBuf>) -> RouterState {
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.join("db"), false).expect("store"));
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("mock"));
    let scheduler = Arc::new(aura_engine::scheduler::Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        dir.join("ws"),
        None,
    ));
    RouterState::new(RouterStateConfig {
        store,
        scheduler,
        config: NodeConfig {
            cloud_node: true,
            data_dir: dir.to_path_buf(),
            workspace,
            ..NodeConfig::default()
        },
        provider,
        tool_config: ToolConfig::default(),
        catalog: Arc::new(ToolCatalog::new()),
        domain_api: None,
        automaton_controller: None,
        automaton_bridge: None,
        memory_manager: None,
        skill_manager: None,
        secrets_vault: None,
        process_store: None,
        trigger_registrar: None,
        router_url: None,
    })
}

fn claim(state: &RouterState) {
    state.cloud.apply_claim(
        NodeClaim {
            node_id: "aura-re36-3f9c".into(),
            node_token: NODE_TOKEN.into(),
            mesh_token: "mesh-token-for-tests-0123".into(),
            seal: Vec::new(),
            owner: None,
            plans: Vec::new(),
            git: None,
        },
        8080,
    );
}

fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A committed workspace with an uncommitted hub edit marked as the base;
/// returns the base commit.
fn workspace_with_hub_base(ws: &std::path::Path) -> String {
    std::fs::create_dir(ws).expect("mkdir");
    git_in(ws, &["init", "-q"]);
    std::fs::write(ws.join("a.rs"), "one").expect("write");
    git_in(ws, &["add", "-A"]);
    git_in(
        ws,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "c",
        ],
    );
    // What the hub sent: an edit, left uncommitted, marked as the base.
    std::fs::write(ws.join("a.rs"), "one, from the hub").expect("write");
    let tree = {
        git_in(ws, &["add", "-A"]);
        let tree = git_in(ws, &["write-tree"]);
        git_in(ws, &["reset", "-q"]);
        tree
    };
    let base = git_in(
        ws,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-m",
            "base",
        ],
    );
    git_in(
        ws,
        &["update-ref", aura_engine_cloud::workspace::BASE_REF, &base],
    );
    base
}

#[tokio::test]
async fn the_patch_holds_only_work_past_the_base_and_leaves_the_index_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("workspace");
    let base = workspace_with_hub_base(&ws);
    // The agent's work: an untracked file and a secret it must not send.
    std::fs::write(ws.join("new.rs"), "agent").expect("write");
    std::fs::write(ws.join(".env"), "TOKEN=1").expect("write");
    let index_before = std::fs::read(ws.join(".git/index")).expect("index");

    let state = cloud_state_in(dir.path(), Some(ws.clone()));
    claim(&state);
    let (status, body) = call(&state, "GET", "/v1/workspace/patch", Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"], serde_json::json!(["new.rs"]), "{body}");
    assert_eq!(body["base_commit"], base.as_str());
    let patch = body["patch"].as_str().unwrap_or_default();
    assert!(
        !patch.contains("from the hub") && !patch.contains("TOKEN"),
        "{patch}"
    );
    let tree = body["tree"].as_str().expect("tree").to_string();
    assert_eq!(git_in(&ws, &["cat-file", "-t", &tree]), "tree");
    assert_eq!(
        std::fs::read(ws.join(".git/index")).expect("index"),
        index_before,
        "the agent's index is untouched"
    );
    assert!(
        std::fs::read_dir(ws.join(".git"))
            .expect("git dir")
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().starts_with("aura-patch-")),
        "the scratch index is gone"
    );
}

async fn call(
    state: &RouterState,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(json) => {
            request = request.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = create_router(state.clone())
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn an_unclaimed_node_refuses_and_a_claimed_one_serves_its_hub() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    let (status, _) = call(&state, "GET", "/v1/host", Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    // The node's own startup token is never accepted on a cloud node.
    let (status, _) = call(&state, "GET", "/v1/host", Some("test"), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    claim(&state);
    let (status, _) = call(&state, "GET", "/v1/host", Some("test"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = call(&state, "GET", "/v1/host", Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["cpu_count"].as_u64().unwrap_or(0) > 0, "{body}");
    let (status, _) = call(&state, "GET", "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
}

async fn post_import(
    state: &RouterState,
    import: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    call(
        state,
        "POST",
        "/v1/sync/import",
        Some(NODE_TOKEN),
        Some(import),
    )
    .await
}

#[tokio::test]
async fn import_appends_verified_entries_and_seal_stops_further_writes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([5; 32]);
    let entries: Vec<RecordEntry> = (1..=3)
        .map(|seq| {
            RecordEntry::builder(seq, Transaction::user_prompt(agent, format!("p{seq}"))).build()
        })
        .collect();
    let import = serde_json::json!({
        "agent_id": agent.to_hex(),
        "name": "re36",
        "entries": entries,
        "done": true,
    });
    let (status, body) = post_import(&state, import.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["head_seq"], 3);
    // Importing the same page again changes nothing.
    let (status, body) = post_import(&state, import.clone()).await;
    assert_eq!(
        (status, body["head_seq"].as_u64()),
        (StatusCode::OK, Some(3))
    );

    let (status, body) = call(
        &state,
        "POST",
        &format!("/v1/sync/seal/{}", agent.to_hex()),
        Some(NODE_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["head_seq"], 3);
    let (status, _) = post_import(&state, import).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let tx =
        serde_json::json!({"agent_id": agent.to_hex(), "kind": "user_prompt", "payload": "aGk="});
    let (status, _) = call(&state, "POST", "/tx", Some(NODE_TOKEN), Some(tx)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_gap_in_an_import_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([6; 32]);
    let entry = RecordEntry::builder(2, Transaction::user_prompt(agent, "p2")).build();
    let import = serde_json::json!({"agent_id": agent.to_hex(), "name": "x", "entries": [entry], "done": true});
    let (status, body) = call(
        &state,
        "POST",
        "/v1/sync/import",
        Some(NODE_TOKEN),
        Some(import),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

#[tokio::test]
async fn mail_from_a_peer_reaches_nobody_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let mail = serde_json::json!({
        "id": 1, "from_node": "aura-ex2-1111", "thread": "aura-ex2-1111:t1", "subject": "s",
        "from": AgentId::new([1; 32]).to_hex(), "from_name": "peer", "to": AgentId::new([2; 32]).to_hex(),
        "kind": "request", "text": "hi", "sent_ms": 0
    });
    let (status, body) = call(
        &state,
        "POST",
        "/v1/mesh/mail",
        Some("mesh-token-for-tests-0123"),
        Some(mail),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "mail_ack");
    assert_eq!(body["delivered"], false);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown agent"),
        "{body}"
    );
}

#[tokio::test]
async fn other_nodes_do_not_serve_cloud_routes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut state = cloud_state(dir.path());
    state.cloud = crate::cloud_node::CloudNode::new(&NodeConfig::default());
    state.config.require_auth = false;
    state.config.cloud_node = false;
    let (status, _) = call(&state, "GET", "/v1/host", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn mesh_mail_maps_to_host_mail() {
    let mail = MeshMail {
        id: 3,
        from_node: "hub".into(),
        thread: "hub:t1".into(),
        subject: "Review".into(),
        from: AgentId::new([1; 32]).to_hex(),
        from_name: "lead".into(),
        to: AgentId::new([2; 32]).to_hex(),
        kind: MeshMailKind::Reply,
        text: "done".into(),
        sent_ms: 0,
    };
    let remote = remote_mail(&mail).expect("maps");
    assert_eq!(remote.kind, MessageKind::Reply);
    assert_eq!(remote.thread, ThreadId("hub:t1".into()));
    assert!(remote_mail(&MeshMail {
        from: "zz".into(),
        ..mail
    })
    .is_err());
}
