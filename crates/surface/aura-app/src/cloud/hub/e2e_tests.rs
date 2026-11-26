//! The hub against a real node served in-process, with fake boxd in
//! front: when a cloud agent's record is copied here, and what moves
//! with the agent.

use super::*;
use crate::cloud::test_node::TestNode;
use aura_agent::checkpoint::{SessionCheckpoint, SessionUsage};
use aura_core_protocol::cloud::{MeshProcess, MESH_PROCESS_AGENT};
use aura_core_types::{RecordEntry, Transaction, TransactionType};
use aura_engine_cloud::{FakeProvider, MachineInfo, MachineStatus};
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use aura_store_db::{RocksStore, TraceStore};

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
            children: Vec::new(),
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
        self.hub.store.get_head_seq(agent).expect("head")
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

#[tokio::test]
async fn the_session_is_kept_here_before_sleep_unless_it_is_ahead_of_the_record() {
    let p = placed(CloudConfig::defaults()).await;
    let sessions = Checkpoints::new(p.hub_dir.path());
    p.hub.attach_sessions(sessions.clone());
    p.write_on_node(1, 2);
    p.save_on_node(3, vec![Message::user("p1")]).await;

    p.hub.copy_before_sleep(p.agent).await;

    assert_eq!(p.local_head(), 2);
    assert!(
        sessions.load_current(p.agent).is_none(),
        "it counts entries the record here doesn't hold"
    );

    p.save_on_node(2, vec![Message::user("p1"), Message::user("p2")])
        .await;
    p.hub.sleep(p.agent).await.expect("slept");

    let kept = sessions
        .load_current(p.agent)
        .expect("kept before the machine slept");
    assert_eq!((kept.record_seq, kept.messages.len()), (2, 2));
    let status = p.hub.sync_status(p.agent).expect("placed");
    assert_eq!(status.copied_seq, 2);
    assert!(status.session_ms.is_some());
    assert!(!status.behind());
}

#[tokio::test]
async fn after_bring_back_the_session_resumes_with_its_tool_calls() {
    let p = placed(CloudConfig::defaults()).await;
    let sessions = Checkpoints::new(p.hub_dir.path());
    p.hub.attach_sessions(sessions.clone());
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
    p.save_on_node(
        2,
        vec![
            Message::user("read notes.txt"),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::tool_use(
                    "t1",
                    "read_file",
                    serde_json::json!({ "path": "notes.txt" }),
                )],
            ),
            Message::tool_results(vec![(
                "t1".into(),
                ToolResultContent::Text("three tasks".into()),
                false,
            )]),
            Message::assistant("it lists three tasks"),
        ],
    )
    .await;
    append(
        p.node.store.as_ref(),
        3,
        Transaction::user_prompt(p.agent, "and the first?"),
    );

    p.hub
        .recall(p.agent, "test", &BootReporter::none())
        .await
        .expect("brought back");

    let resumed = aura_agent::history::restore(p.hub.store.as_ref(), &sessions, p.agent)
        .expect("resumes from the copied session");
    assert!(resumed.messages[1].has_tool_use(), "with its tool call");
    assert!(
        texts(&resumed.messages).contains(&(Role::User, "and the first?".to_string())),
        "and what the record holds past it"
    );
}

#[tokio::test]
async fn spawned_agents_are_copied_only_when_the_record_names_them() {
    let p = placed(CloudConfig::defaults()).await;
    let child = AgentId::new([7; 32]);
    let stranger = AgentId::new([8; 32]);
    let known = AgentId::new([9; 32]);
    append(
        p.hub.store.as_ref(),
        1,
        Transaction::user_prompt(known, "mine"),
    );
    append(
        p.node.store.as_ref(),
        1,
        Transaction::user_prompt(p.agent, "split the work"),
    );
    append(p.node.store.as_ref(), 2, spawned(p.agent, child));
    append(p.node.store.as_ref(), 3, spawned(p.agent, known));
    for agent in [child, stranger, known] {
        for seq in 1..=2 {
            append(
                p.node.store.as_ref(),
                seq,
                Transaction::user_prompt(agent, format!("p{seq}")),
            );
        }
    }

    assert_eq!(p.hub.mirror(p.agent).await.expect("copied"), 5);

    assert_eq!(p.head_of(child), 2, "named by the record: copied");
    assert_eq!(p.head_of(stranger), 0, "never named: left on the machine");
    assert_eq!(p.head_of(known), 1, "already has a record here: not taken");
    assert_eq!(p.hub.sync_status(p.agent).expect("placed").children, 1);
    let saved = std::fs::read_to_string(p.hub_dir.path().join("cloud/placements.json"))
        .expect("placements");
    assert!(saved.contains(&child.to_hex()) && !saved.contains(&known.to_hex()));

    append(
        p.node.store.as_ref(),
        3,
        Transaction::user_prompt(child, "p3"),
    );
    assert_eq!(p.hub.mirror(p.agent).await.expect("copied"), 1);
    assert_eq!(p.head_of(child), 3, "and kept up with");
}

#[tokio::test]
async fn a_turn_s_run_is_copied_into_the_main_trace_store() {
    let p = placed(CloudConfig::defaults()).await;
    let db = RocksStore::open(p.hub_dir.path().join("traces"), false).expect("open");
    let traces = Arc::new(TraceStore::for_store(&db));
    p.hub.attach_traces(Arc::clone(&traces));
    p.import().await;

    let mut events = p.hub.run_turn(
        p.agent,
        "hello".into(),
        Some("mock-model".into()),
        None,
        None,
        CancellationToken::new(),
    );
    while let Some(event) = events.recv().await {
        if let RemoteEvent::Failed(err) = event {
            panic!("the turn failed: {err}");
        }
    }

    let run = aura_agent_trace::db::latest_of(&traces, &p.agent.to_hex())
        .expect("read")
        .expect("the turn's run is here");
    assert_eq!(
        run.meta.agent_id.as_deref(),
        Some(p.agent.to_hex().as_str())
    );
    assert!(!run.steps.is_empty(), "with its steps");
    assert!(p.hub.take_traces_copied());
    assert!(p
        .hub
        .sync_status(p.agent)
        .expect("placed")
        .traces_ms
        .is_some());
}

#[tokio::test]
async fn bring_back_uses_the_changes_saved_at_sleep_when_the_machine_is_gone() {
    let p = placed(CloudConfig::defaults()).await;
    let project = p.hub_dir.path().join("project");
    let workspace = p.node_dir.path().join("ws");
    repo(&project, "one\n");
    repo(&workspace, "one\n");
    std::fs::write(workspace.join("notes.txt"), "one\ntwo\n").expect("changed");

    p.hub.sleep(p.agent).await.expect("slept");
    let hex = p.agent.to_hex();
    assert!(p.hub.saved_patch(&hex).is_some(), "saved before it slept");
    p.hub.fleet.destroy(&hex, "lost").await.expect("destroyed");
    assert!(p.hub.fleet.view(&hex).is_none());

    let outcome = p
        .hub
        .recall(p.agent, "test", &BootReporter::none())
        .await
        .expect("brought back");

    assert!(
        matches!(&outcome.patch, PatchOutcome::Applied { files } if files == &["notes.txt"]),
        "{:?}",
        outcome.patch
    );
    assert_eq!(
        std::fs::read_to_string(project.join("notes.txt")).expect("notes"),
        "one\ntwo\n"
    );
    assert!(!p.hub.is_placed(p.agent));
    assert!(p.hub.saved_patch(&hex).is_none(), "dropped once used");
}

#[tokio::test]
async fn a_pull_drops_the_changes_saved_at_sleep() {
    let p = placed(CloudConfig::defaults()).await;
    let project = p.hub_dir.path().join("project");
    let workspace = p.node_dir.path().join("ws");
    repo(&project, "one\n");
    repo(&workspace, "one\n");
    std::fs::write(workspace.join("notes.txt"), "one\ntwo\n").expect("changed");
    p.hub.sleep(p.agent).await.expect("slept");
    let hex = p.agent.to_hex();
    assert!(p.hub.saved_patch(&hex).is_some(), "saved before it slept");

    let outcome = p.hub.pull(p.agent).await.expect("pulled");

    assert!(
        matches!(&outcome.patch, PatchOutcome::Applied { .. }),
        "{:?}",
        outcome.patch
    );
    assert!(p.hub.saved_patch(&hex).is_none(), "in the project now");
    let sync = p.hub.sync_status(p.agent).expect("placed");
    assert!(sync.patch_ms.is_none(), "the next sleep saves a new one");
}

#[tokio::test]
async fn the_machine_titles_the_chat_during_the_turn_and_records_the_call() {
    let p = placed(CloudConfig::defaults()).await;
    p.import().await;

    let mut events = p.hub.run_turn(
        p.agent,
        "fix the build".into(),
        Some("mock-model".into()),
        Some("mock-model".into()),
        None,
        CancellationToken::new(),
    );
    let (mut title, mut done) = (None, false);
    while let Some(event) = events.recv().await {
        match event {
            RemoteEvent::Title(written) => {
                assert!(!done, "the title comes while the turn runs");
                title = Some(written);
            }
            RemoteEvent::Done(_) => done = true,
            RemoteEvent::Failed(err) => panic!("the turn failed: {err}"),
            _ => {}
        }
    }

    assert!(done);
    assert_eq!(title.as_deref(), Some("done"), "the node model's answer");
    let copied = p.hub.store.scan_record(p.agent, 1, 1_000).expect("scan");
    assert!(
        copied.iter().any(|entry| {
            entry.tx.tx_type == TransactionType::Reasoning
                && serde_json::from_slice::<serde_json::Value>(&entry.tx.payload)
                    .is_ok_and(|call| call["purpose"] == "title")
        }),
        "the title call is in the record copied here"
    );
}

#[tokio::test]
async fn a_renamed_cloud_agent_is_named_so_in_its_placement_and_the_directory() {
    let p = placed(CloudConfig::defaults()).await;
    let hex = p.agent.to_hex();
    p.hub.directory.place(
        &hex,
        Holder {
            node: MACHINE.into(),
            url: None,
            project: "p1".into(),
            name: "re36".into(),
            owner: "local".into(),
        },
    );

    p.hub.rename(p.agent, "Build fix");

    assert_eq!(p.hub.placements.lock().agents[&hex].name, "Build fix");
    let saved = std::fs::read_to_string(p.hub_dir.path().join("cloud/placements.json"))
        .expect("placements saved");
    assert!(saved.contains("Build fix"), "{saved}");
    assert_eq!(
        p.hub.directory.holder(&hex).map(|holder| holder.name),
        Some("Build fix".to_string())
    );
    p.hub.rename(AgentId::new([1; 32]), "Nobody");
    assert!(
        !p.hub.is_placed(AgentId::new([1; 32])),
        "only placed agents"
    );
}

/// A row of a machine's PROCESSES frame: a turn, or with a `parent` an
/// agent spawned there.
fn process(id: &str, agent: &str, status: &str, parent: Option<&str>) -> MeshProcess {
    MeshProcess {
        id: id.into(),
        agent_id: agent.into(),
        label: id.into(),
        status: status.into(),
        started_ms: 1,
        kind: parent.map(|_| MESH_PROCESS_AGENT.into()),
        run_id: parent.is_none().then(|| "run-1".into()),
        parent: parent.map(str::to_string),
        name: parent.map(|_| "helper".into()),
    }
}

#[tokio::test]
async fn the_turn_streamed_from_here_is_not_also_the_machine_s() {
    let p = placed(CloudConfig::defaults()).await;
    let hex = p.agent.to_hex();
    let streamed = p.hub.streaming_turn(&hex, "run-1".into());
    p.hub
        .note_processes(&hex, vec![process("turn:run-1:1", &hex, "running", None)]);
    assert!(
        p.hub.machine_processes().is_empty(),
        "shown as its own turn"
    );

    drop(streamed);
    p.hub.note_processes(
        &hex,
        vec![
            process("turn:run-1:1", &hex, "running", None),
            process("turn:run-1:2", &hex, "running", None),
        ],
    );
    let ids: Vec<String> = p
        .hub
        .machine_processes()
        .into_iter()
        .flat_map(|(_, rows)| rows.into_iter().map(|row| row.id))
        .collect();
    assert_eq!(
        ids,
        ["turn:run-1:2"],
        "the streamed turn stays its own until it ends; the next one in the run is the machine's"
    );
}

#[tokio::test]
async fn work_a_machine_reports_keeps_it_awake_and_shows_only_its_agent_s() {
    let p = placed(CloudConfig {
        idle_hibernate_secs: 1,
        ..CloudConfig::defaults()
    })
    .await;
    let hex = p.agent.to_hex();
    let helper = AgentId::new([7; 32]).to_hex();
    let grandchild = AgentId::new([8; 32]).to_hex();
    let known = AgentId::new([9; 32]);
    append(
        p.hub.store.as_ref(),
        1,
        reply(known, "an agent of this computer"),
    );

    p.hub.note_processes(
        &hex,
        vec![
            process("turn:r1", &hex, "running", None),
            process("turn:r2", &AgentId::new([5; 32]).to_hex(), "running", None),
            process("agent:h", &helper, "idle", Some(&hex)),
            process("agent:g", &grandchild, "running", Some(&helper)),
            process("agent:k", &known.to_hex(), "running", Some(&hex)),
            process(
                "agent:o",
                &AgentId::new([4; 32]).to_hex(),
                "running",
                Some("ff"),
            ),
        ],
    );

    let shown = p.hub.machine_processes();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, hex);
    let ids: Vec<&str> = shown[0].1.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, ["turn:r1", "agent:h", "agent:g"]);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    p.hub.sleep_idle_machines().await;
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Running,
        "kept awake while its work runs"
    );

    p.hub
        .note_processes(&hex, vec![process("agent:h", &helper, "idle", Some(&hex))]);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    p.hub.sleep_idle_machines().await;
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Hibernated
    );

    p.hub.forget_processes(&hex);
    assert!(p.hub.machine_processes().is_empty());
}
