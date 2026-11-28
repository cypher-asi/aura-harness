//! The hub against a real node served in-process, with fake boxd in
//! front: when a cloud agent's record is copied here, and what moves
//! with the agent.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aura_agent::checkpoint::{Checkpoints, SessionCheckpoint, SessionUsage};
use aura_agent_kernel::RecordObserver;
use aura_config::CloudConfig;
use aura_core_protocol::cloud::{MeshProcess, MESH_PROCESS_AGENT};
use aura_core_types::{AgentId, RecordEntry, Transaction, TransactionType};
use aura_engine_cloud::fake::ExecHandler;
use aura_engine_cloud::hub::test_support as hooks;
use aura_engine_cloud::hub::{Owner, PlacedAgent};
use aura_engine_cloud::machine_env::WORKSPACE_DIR;
use aura_engine_cloud::mesh::Holder;
use aura_engine_cloud::snapshots::AuraBinary;
use aura_engine_cloud::{BootReporter, FakeProvider, MachineInfo, MachineStatus};
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use aura_store_db::{RocksStore, Store, TraceStore};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::test_node::TestNode;
use super::{CloudHub, PatchOutcome, PlaceRequest, RemoteEvent};

mod machine;
mod record;
mod session;

const MACHINE: &str = "aura-re36-0001";
const ZNS: &str = "0://terminal/re36";

/// A hub whose agent's machine runs `node`, claimed and awake.
struct Placed {
    hub: Arc<CloudHub>,
    fake: FakeProvider,
    node: TestNode,
    agent: AgentId,
    hub_dir: tempfile::TempDir,
    node_dir: tempfile::TempDir,
}

fn machine(base: &str) -> MachineInfo {
    MachineInfo {
        id: "m-0001".into(),
        name: MACHINE.into(),
        status: MachineStatus::Running,
        region: String::new(),
        vcpu: 2,
        memory_bytes: 4 << 30,
        disk_bytes: 20 << 30,
        domain: base.to_string(),
        public_ip: String::new(),
        ssh_port: 0,
        isolated: false,
        labels: BTreeMap::new(),
        source: None,
        created_ms: 0,
        boot_ms: 0,
    }
}

/// The agent's placement on [`MACHINE`], its project at `project_dir`.
fn placement(project_dir: PathBuf) -> PlacedAgent {
    PlacedAgent {
        zns: ZNS.into(),
        name: "re36".into(),
        project: "p1".into(),
        project_dir,
        machine: MACHINE.into(),
        owner: None,
        owner_name: None,
        branch: None,
        repo: None,
        fetched_tip: None,
        children: Vec::new(),
    }
}

async fn placed(config: CloudConfig) -> Placed {
    placed_with(config, TestNode::exec_handler).await
}

/// [`placed`], what fake boxd runs in the machine being `exec(node)`.
async fn placed_with(config: CloudConfig, exec: fn(&TestNode) -> ExecHandler) -> Placed {
    let hub_dir = tempfile::tempdir().expect("tempdir");
    let node_dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn Store> =
        Arc::new(RocksStore::open(hub_dir.path().join("db"), false).expect("open"));
    let fake = FakeProvider::new();
    let hub = CloudHub::new(
        config,
        store,
        hub_dir.path().join("cloud"),
        Arc::new(fake.clone()),
        AuraBinary::Installed,
    )
    .expect("hub");
    let node = TestNode::serve(node_dir.path(), "done").await;
    fake.on_exec(exec(&node));
    fake.insert_machine(machine(&node.base));
    let agent = AgentId::new([6; 32]);
    let hex = agent.to_hex();
    hub.fleet()
        .adopt(&hex, ZNS, "p1", MACHINE, Vec::new(), None)
        .await
        .expect("adopted");
    hub.fleet().ensure_awake(&hex).await.expect("claimed");
    let project_dir = hub_dir.path().join("project");
    std::fs::create_dir_all(&project_dir).expect("project");
    hooks::insert_placement(&hub, hex, placement(project_dir));
    Placed {
        hub,
        fake,
        node,
        agent,
        hub_dir,
        node_dir,
    }
}

impl Placed {
    fn request(&self) -> PlaceRequest {
        PlaceRequest {
            agent: self.agent,
            agent_zns: ZNS.into(),
            name: "re36".into(),
            code: Some("RE36".into()),
            project_id: "p1".into(),
            project_dir: self.hub_dir.path().join("project"),
            build_command: None,
            fork_from: None,
            owner: Owner::Local,
            owner_name: None,
            clean: false,
        }
    }

    /// Hand the node the local record, as a placement does.
    async fn import(&self) {
        let view = self
            .hub
            .fleet()
            .view(&self.agent.to_hex())
            .expect("machine");
        hooks::import_record(&self.hub, &self.request(), &view, &BootReporter::none())
            .await
            .expect("imported");
    }

    /// Prompts `from..=to` on the node, as its kernel writes them.
    fn write_on_node(&self, from: u64, to: u64) {
        for seq in from..=to {
            append(
                self.node.store.as_ref(),
                seq,
                Transaction::user_prompt(self.agent, format!("p{seq}")),
            );
        }
    }

    fn local_head(&self) -> u64 {
        hooks::store(&self.hub)
            .get_head_seq(self.agent)
            .expect("head")
    }

    /// Whether the local copy reaches `head` within five seconds.
    async fn copied_up_to(&self, head: u64) -> bool {
        for _ in 0..100 {
            if self.local_head() >= head {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// The node's session checkpoint, taken at `record_seq`.
    async fn save_on_node(&self, record_seq: u64, messages: Vec<Message>) {
        Checkpoints::new(self.node_dir.path())
            .save(
                self.agent,
                &SessionCheckpoint::new(
                    1_000,
                    record_seq,
                    messages,
                    Vec::new(),
                    SessionUsage::default(),
                ),
            )
            .await
            .expect("saved");
    }

    fn head_of(&self, agent: AgentId) -> u64 {
        hooks::store(&self.hub).get_head_seq(agent).expect("head")
    }
}

/// `parent`'s record row for spawning `child`, as its kernel writes it.
fn spawned(parent: AgentId, child: AgentId) -> Transaction {
    let payload = serde_json::json!({
        "kind": "spawn_agent",
        "parent_agent_id": parent,
        "child_agent_id": child,
        "name": "helper",
    });
    Transaction::new_chained(
        parent,
        TransactionType::System,
        serde_json::to_vec(&payload).expect("payload"),
        None,
    )
}

/// A git repository at `dir` with `notes.txt` holding `text`, committed.
fn repo(dir: &Path, text: &str) {
    std::fs::create_dir_all(dir).expect("dir");
    git(dir, &["init", "-q"]);
    std::fs::write(dir.join("notes.txt"), text).expect("notes");
    git(dir, &["add", "."]);
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-q",
            "-m",
            "base",
        ],
    );
}

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

fn append(store: &dyn Store, seq: u64, tx: Transaction) {
    let agent = tx.agent_id;
    let entry = RecordEntry::builder(seq, tx).build();
    store
        .append_entry_direct(agent, seq, &entry)
        .expect("append");
}

fn reply(agent: AgentId, text: &str) -> Transaction {
    Transaction::new_chained(
        agent,
        TransactionType::AgentMsg,
        text.as_bytes().to_vec(),
        None,
    )
}

fn texts(messages: &[Message]) -> Vec<(Role, String)> {
    messages
        .iter()
        .map(|m| (m.role, m.text_content()))
        .collect()
}
