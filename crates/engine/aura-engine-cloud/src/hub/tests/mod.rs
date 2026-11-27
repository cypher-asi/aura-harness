use super::*;

use crate::machine_env::WORKSPACE_DIR;
use crate::mesh::Holder;
use crate::{names, workspace, BootReporter, FakeProvider};

use aura_core_protocol::cloud::{MeshMail, MeshMailKind};
use aura_core_protocol::{ErrorMsg, OutboundMessage, TextDelta, ToolUseStart};
use aura_core_types::time::now_ms;
use aura_engine::{MessageKind, RemoteMail, RemoteRouter, ThreadId};

use aura_store_db::RocksStore;

fn hub() -> (Arc<CloudHub>, FakeProvider, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn Store> =
        Arc::new(RocksStore::open(dir.path().join("db"), false).expect("open"));
    let fake = FakeProvider::new();
    let hub = CloudHub::new(
        CloudConfig::defaults(),
        store,
        dir.path().join("cloud"),
        Arc::new(fake.clone()),
        AuraBinary::Installed,
    )
    .expect("hub");
    (hub, fake, dir)
}

fn place_cloud(hub: &CloudHub, agent: AgentId, node: &str, owner: &Owner) {
    hub.directory.place(
        &agent.to_hex(),
        Holder {
            node: node.into(),
            url: Some(format!("https://{node}.boxd.sh")),
            project: "p1".into(),
            name: format!("agent on {node}"),
            owner: names::owner_tag(owner),
        },
    );
}

fn test_git(dir: &Path, args: &[&str]) -> String {
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

fn test_commit(dir: &Path, file: &str, message: &str) -> String {
    std::fs::write(dir.join(file), message).expect("write");
    test_git(dir, &["add", "-A"]);
    test_git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
    test_git(dir, &["rev-parse", "HEAD"])
}

/// A project with one commit and `origin`, if any.
fn project_repo(dir: &Path, origin: Option<&str>) {
    std::fs::create_dir_all(dir).expect("mkdir");
    test_git(dir, &["init", "-q", "-b", "main"]);
    if let Some(url) = origin {
        test_git(dir, &["remote", "add", "origin", url]);
    }
    test_commit(dir, "a.txt", "first");
}

fn request_in(project: &Path) -> PlaceRequest {
    PlaceRequest {
        agent: AgentId::new([1; 32]),
        agent_zns: "0://scout".into(),
        name: "scout".into(),
        code: Some("SC01".into()),
        project_id: "p1".into(),
        project_dir: project.to_path_buf(),
        build_command: None,
        fork_from: None,
        owner: Owner::Local,
        owner_name: None,
    }
}

/// Answer the fake's execs by running them against `workspace`, with
/// `/tmp/` under `root`, and hand back the bundle they leave.
fn run_scripts_in(fake: &FakeProvider, root: &Path, workspace_dir: &Path) {
    let files = fake.clone();
    let root = root.to_path_buf();
    let workspace_dir = workspace_dir.to_path_buf();
    fake.on_exec(Arc::new(move |machine, command, _stdin| {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command.replace("/tmp/", &format!("{}/", root.display())))
            .env("AURA_WORKSPACE", &workspace_dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("sh");
        if let Ok(bytes) = std::fs::read(root.join("aura-out.bundle")) {
            files.put_file(machine, workspace::OUT_BUNDLE, bytes);
        }
        crate::ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        }
    }));
}

/// Place `agent` on a fake machine as a restart would find it.
async fn adopt_machine(
    hub: &CloudHub,
    fake: &FakeProvider,
    agent: AgentId,
    project: &Path,
    branch: Option<&str>,
) -> PlacedAgent {
    use crate::{MachineInfo, MachineStatus};
    let name = "aura-sc01-0101";
    fake.insert_machine(MachineInfo {
        id: "vm-1".into(),
        name: name.into(),
        status: MachineStatus::Running,
        region: String::new(),
        vcpu: 4,
        memory_bytes: 0,
        disk_bytes: 0,
        domain: format!("{name}.boxd.sh"),
        public_ip: String::new(),
        ssh_port: 0,
        isolated: true,
        labels: std::collections::BTreeMap::new(),
        source: None,
        created_ms: 0,
        boot_ms: 0,
    });
    hub.fleet
        .adopt(&agent.to_hex(), "0://scout", "p1", name, Vec::new(), None)
        .await
        .expect("adopt");
    let placed = PlacedAgent {
        zns: "0://scout".into(),
        name: "scout".into(),
        project: "p1".into(),
        project_dir: project.to_path_buf(),
        machine: name.into(),
        owner: None,
        owner_name: None,
        branch: branch.map(str::to_string),
        repo: None,
        fetched_tip: None,
        children: Vec::new(),
    };
    hub.placements
        .lock()
        .agents
        .insert(agent.to_hex(), placed.clone());
    placed
}

fn alice() -> Owner {
    Owner::Zos {
        user_id: "alice".into(),
    }
}

fn bob() -> Owner {
    Owner::Zos {
        user_id: "bob".into(),
    }
}

fn mail_to(from: AgentId, from_node: &str, to: AgentId, id: u64) -> MeshMail {
    MeshMail {
        id,
        from_node: from_node.into(),
        thread: format!("{from_node}:t1"),
        subject: "s".into(),
        from: from.to_hex(),
        from_name: "scout".into(),
        to: to.to_hex(),
        kind: MeshMailKind::Request,
        text: "hi".into(),
        sent_ms: 0,
    }
}

fn ack_error(frame: MeshFrame) -> Option<String> {
    let MeshFrame::MailAck {
        delivered, error, ..
    } = frame
    else {
        panic!("not an ack");
    };
    (!delivered).then(|| error.unwrap_or_default())
}

mod boundaries;
mod github;
mod mail;
mod spawns;
mod wire;
