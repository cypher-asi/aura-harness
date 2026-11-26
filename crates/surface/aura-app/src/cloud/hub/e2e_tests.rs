//! The hub against a real node served in-process, with fake boxd in
//! front: when a cloud agent's record is copied here, and what moves
//! with the agent.

use super::*;
use crate::cloud::test_node::TestNode;
use aura_agent::checkpoint::{SessionCheckpoint, SessionUsage};
use aura_core_types::{RecordEntry, Transaction, TransactionType};
use aura_engine_cloud::{FakeProvider, MachineInfo, MachineStatus};
use aura_model::{Message, Role};
use aura_store_db::RocksStore;

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

async fn placed(config: CloudConfig) -> Placed {
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
    fake.on_exec(node.exec_handler());
    fake.insert_machine(machine(&node.base));
    let agent = AgentId::new([6; 32]);
    let hex = agent.to_hex();
    hub.fleet
        .adopt(&hex, ZNS, "p1", MACHINE, Vec::new(), None)
        .await
        .expect("adopted");
    hub.fleet.ensure_awake(&hex).await.expect("claimed");
    let project_dir = hub_dir.path().join("project");
    std::fs::create_dir_all(&project_dir).expect("project");
    hub.placements.lock().agents.insert(
        hex,
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
        },
    );
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
        }
    }

    /// Hand the node the local record, as a placement does.
    async fn import(&self) {
        let view = self.hub.fleet.view(&self.agent.to_hex()).expect("machine");
        self.hub
            .import_record(&self.request(), &view, &BootReporter::none())
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
        self.hub.store.get_head_seq(self.agent).expect("head")
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

#[tokio::test]
async fn an_idle_machine_is_copied_before_it_sleeps() {
    let p = placed(CloudConfig {
        idle_hibernate_secs: 1,
        ..CloudConfig::defaults()
    })
    .await;
    p.write_on_node(1, 3);
    tokio::time::sleep(Duration::from_millis(1100)).await;

    p.hub.sleep_idle_machines().await;

    assert_eq!(p.local_head(), 3, "copied before it slept");
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Hibernated
    );
}

#[tokio::test]
async fn sleeping_a_machine_by_hand_copies_first() {
    let p = placed(CloudConfig::defaults()).await;
    p.write_on_node(1, 2);

    p.hub.sleep(p.agent).await.expect("slept");

    assert_eq!(p.local_head(), 2);
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Hibernated
    );
}

#[tokio::test]
async fn an_opened_link_catches_up_then_follows_the_node() {
    let p = placed(CloudConfig::defaults()).await;
    p.import().await;
    p.write_on_node(1, 2);

    p.hub.open_link(&p.agent.to_hex());
    assert!(p.copied_up_to(2).await, "caught up when the link opened");

    p.write_on_node(3, 4);
    assert!(
        p.copied_up_to(4).await,
        "the node's hint brought the new entries"
    );
    p.hub.stop.cancel();
}

#[tokio::test]
async fn hints_while_a_copy_runs_fold_into_one_more() {
    let p = placed(CloudConfig::defaults()).await;
    p.write_on_node(1, 2);
    let gate = Arc::clone(p.hub.mirrors.lock().entry(p.agent).or_default());
    let held = gate.lock().await;

    for _ in 0..10 {
        p.hub.copy_soon(p.agent);
    }
    assert_eq!(
        p.hub.copies.lock().get(&p.agent),
        Some(&true),
        "one copy waits, one more is owed"
    );
    drop(held);

    assert!(p.copied_up_to(2).await);
    for _ in 0..100 {
        if p.hub.copies.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(p.hub.copies.lock().is_empty(), "no copy left running");
}

#[derive(Debug, Default)]
struct Seen(Mutex<Vec<(AgentId, u64)>>);

impl RecordObserver for Seen {
    fn on_entry(&self, entry: &RecordEntry) {
        self.0.lock().push((entry.tx.agent_id, entry.seq));
    }
}

#[tokio::test]
async fn copied_entries_reach_the_observer_in_order() {
    let p = placed(CloudConfig::defaults()).await;
    let seen = Arc::new(Seen::default());
    p.hub
        .attach_observer(Arc::clone(&seen) as Arc<dyn RecordObserver>);
    p.write_on_node(1, 3);
    assert!(!p.hub.take_copied());

    assert_eq!(p.hub.mirror(p.agent).await.expect("copied"), 3);

    assert_eq!(
        *seen.0.lock(),
        vec![(p.agent, 1), (p.agent, 2), (p.agent, 3)]
    );
    assert!(p.hub.take_copied(), "the agent lists are told");
    assert!(!p.hub.take_copied(), "once");
}

#[tokio::test]
async fn placement_hands_the_node_the_conversation() {
    let p = placed(CloudConfig::defaults()).await;
    append(
        p.hub.store.as_ref(),
        1,
        Transaction::user_prompt(p.agent, "read notes.txt"),
    );
    append(
        p.hub.store.as_ref(),
        2,
        reply(p.agent, "it lists three tasks"),
    );
    let sessions = Checkpoints::new(p.hub_dir.path());
    let messages = vec![
        Message::user("read notes.txt"),
        Message::assistant("it lists three tasks"),
    ];
    sessions
        .save(
            p.agent,
            &SessionCheckpoint::new(
                1_000,
                2,
                messages.clone(),
                Vec::new(),
                SessionUsage::default(),
            ),
        )
        .await
        .expect("saved");
    p.hub.attach_sessions(sessions);

    p.import().await;

    let node = p.hub.node(&p.agent.to_hex()).expect("node");
    assert_eq!(node.head(p.agent).await.expect("head"), 2);
    let taken = Checkpoints::new(p.node_dir.path())
        .load_current(p.agent)
        .expect("the node holds the session");
    assert_eq!((taken.started_ms, taken.record_seq), (1_000, 2));
    assert_eq!(texts(&taken.messages), texts(&messages));
}

#[tokio::test]
async fn bringing_an_agent_back_keeps_what_it_said_on_its_machine() {
    let p = placed(CloudConfig::defaults()).await;
    p.import().await;
    append(
        p.node.store.as_ref(),
        1,
        Transaction::user_prompt(p.agent, "read notes.txt"),
    );
    append(
        p.node.store.as_ref(),
        2,
        reply(p.agent, "it lists three tasks"),
    );

    p.hub
        .recall(p.agent, "test", &BootReporter::none())
        .await
        .expect("brought back");

    assert_eq!(p.local_head(), 2);
    assert!(!p.hub.is_placed(p.agent));
    assert!(p.fake.machine(MACHINE).is_none(), "its machine is gone");
    let history =
        aura_agent::history::model_history(p.hub.store.as_ref(), p.agent).expect("history");
    assert_eq!(
        texts(&history),
        [
            (Role::User, "read notes.txt".to_string()),
            (Role::Assistant, "it lists three tasks".to_string()),
        ]
    );
}
