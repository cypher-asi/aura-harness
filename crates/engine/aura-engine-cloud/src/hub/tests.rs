
use super::*;
use crate::FakeProvider;
use aura_core_protocol::{ErrorMsg, TextDelta, ToolUseStart};
use aura_store_db::RocksStore;

#[test]
fn choose_binary_prefers_env_then_cloud_home_then_running_exe() {
    let home = tempfile::tempdir().expect("tempdir");
    let exe = PathBuf::from("/usr/bin/aura-running");
    let env = PathBuf::from("/opt/aura-linux");

    // Nothing built in the cloud home: the running executable.
    assert_eq!(
        choose_binary(None, home.path(), Some(exe.clone())),
        AuraBinary::Upload(exe.clone())
    );
    assert_eq!(
        choose_binary(None, home.path(), None),
        AuraBinary::Installed
    );

    // A build in the cloud home wins over the running executable.
    let built = crate::snapshots::cloud_binary_path(home.path());
    std::fs::create_dir_all(built.parent().expect("parent")).expect("mkdir");
    std::fs::write(&built, b"elf").expect("write");
    assert_eq!(
        choose_binary(None, home.path(), Some(exe.clone())),
        AuraBinary::Upload(built)
    );

    // AURA_CLOUD_BINARY wins over both.
    assert_eq!(
        choose_binary(Some(&env), home.path(), Some(exe)),
        AuraBinary::Upload(env)
    );
}

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

#[test]
fn stream_frames_map_to_events() {
    assert_eq!(
        map_outbound(OutboundMessage::TextDelta(TextDelta { text: "hi".into() })),
        Some(RemoteEvent::Text("hi".into()))
    );
    assert_eq!(
        map_outbound(OutboundMessage::ToolUseStart(ToolUseStart {
            id: "1".into(),
            name: "read_file".into()
        })),
        Some(RemoteEvent::ToolStart {
            id: "1".into(),
            name: "read_file".into()
        })
    );
    let fatal = OutboundMessage::Error(ErrorMsg {
        code: "x".into(),
        message: "boom".into(),
        recoverable: false,
        support_id: None,
    });
    assert_eq!(
        map_outbound(fatal),
        Some(RemoteEvent::Failed("boom".into()))
    );
    let soft = OutboundMessage::Error(ErrorMsg {
        code: "x".into(),
        message: "retry".into(),
        recoverable: true,
        support_id: None,
    });
    assert_eq!(map_outbound(soft), None);
}

#[test]
fn urls_and_requests() {
    assert_eq!(
        ws_url("https://m.boxd.sh", "/v1/mesh"),
        "wss://m.boxd.sh/v1/mesh"
    );
    assert_eq!(
        ws_url("http://127.0.0.1:8080", "/stream/1"),
        "ws://127.0.0.1:8080/stream/1"
    );
    let request = chat_request(AgentId::new([1; 32]), Some("gate".into()), None);
    assert_eq!(
        request.agent_identity.partition_id,
        Some(AgentId::new([1; 32]).to_hex())
    );
    assert_eq!(
        request.workspace.project_path.as_deref(),
        Some(WORKSPACE_DIR)
    );
    let json = serde_json::to_string(&request).expect("json");
    assert!(!json.contains("bxd_"), "{json}");
}

#[test]
fn project_defaults_persist() {
    let (hub, _fake, dir) = hub();
    assert_eq!(hub.project_default("p1"), Placement::Local);
    hub.set_project_default("p1", Placement::Cloud);
    drop(hub);
    let store: Arc<dyn Store> =
        Arc::new(RocksStore::open(dir.path().join("db2"), false).expect("open"));
    let again = CloudHub::new(
        CloudConfig::defaults(),
        store,
        dir.path().join("cloud"),
        Arc::new(FakeProvider::new()),
        AuraBinary::Installed,
    )
    .expect("hub");
    assert_eq!(again.project_default("p1"), Placement::Cloud);
}

#[test]
fn mail_for_a_local_agent_without_a_host_is_refused_and_resends_are_retried() {
    let (hub, _fake, _dir) = hub();
    place_cloud(&hub, AgentId::new([2; 32]), "aura-x", &Owner::Local);
    let mail = MeshMail {
        id: 1,
        from_node: "aura-x".into(),
        thread: "aura-x:t1".into(),
        subject: "s".into(),
        from: AgentId::new([2; 32]).to_hex(),
        from_name: "scout".into(),
        to: AgentId::new([3; 32]).to_hex(),
        kind: MeshMailKind::Request,
        text: "hi".into(),
        sent_ms: 0,
    };
    let MeshFrame::MailAck { delivered, .. } = hub.take_mail(&mail) else {
        panic!("not an ack");
    };
    assert!(!delivered);
    let MeshFrame::MailAck { delivered, .. } = hub.take_mail(&mail) else {
        panic!("not an ack");
    };
    assert!(
        !delivered,
        "a failed delivery is forgotten, so the resend is tried again"
    );
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

#[tokio::test]
async fn a_project_on_github_gives_its_machine_a_repository_secret_and_the_bot() {
    let (hub, fake, dir) = hub();
    let tokens = Arc::new(crate::github::FakeRepoTokens::new("aura-agents"));
    let repo = RepoRef {
        owner: "acme".into(),
        name: "proj".into(),
    };
    tokens.install(&repo, "ghs_secret", now_ms() + 3_600_000);
    hub.set_repo_tokens(Some(tokens));

    let project = dir.path().join("project");
    project_repo(&project, Some("git@github.com:acme/proj.git"));
    let share = hub
        .share_repo(&request_in(&project), &BootReporter::none())
        .await
        .expect("share");
    assert_eq!(share.repo, Some(repo.clone()));
    let secret = share.secret.clone().expect("secret");
    assert!(names::is_repo_secret(&secret), "{secret}");
    assert_eq!(fake.secret_value(&secret).as_deref(), Some("ghs_secret"));
    let claim = CloudHub::git_claim(
        Some("aura/scout-sc01"),
        share.repo.as_ref(),
        share.secret.clone(),
        share.identity.as_ref(),
    )
    .expect("claim");
    assert_eq!(claim.branch, "aura/scout-sc01");
    assert_eq!(
        claim.remote_url.as_deref(),
        Some("https://github.com/acme/proj.git")
    );
    assert_eq!(claim.env.as_deref(), Some(secret.as_str()));
    assert_eq!(claim.placeholder, None);
    assert_eq!(claim.author_name.as_deref(), Some("aura-agents[bot]"));
    assert_eq!(
        claim.author_email.as_deref(),
        Some("41000000+aura-agents[bot]@users.noreply.github.com")
    );
    assert!(hub.repo_status(&repo).is_some_and(|status| status.linked));

    // Not installed: the secret holds no token, and placing goes on.
    let other = dir.path().join("other");
    project_repo(&other, Some("https://github.com/acme/other.git"));
    let share = hub
        .share_repo(&request_in(&other), &BootReporter::none())
        .await
        .expect("share");
    let secret = share.secret.expect("secret");
    assert_eq!(
        fake.secret_value(&secret).as_deref(),
        Some(crate::repos::UNLINKED)
    );
    let status = hub
        .repo_status(share.repo.as_ref().expect("repo"))
        .expect("status");
    assert!(!status.linked);
    assert_eq!(
        status.install_url.as_deref(),
        Some("https://github.com/apps/aura-agents/installations/new")
    );

    // No origin, or one off GitHub: no secret, and still a branch.
    for (name, origin) in [
        ("local", None),
        ("gitlab", Some("https://gitlab.com/acme/proj.git")),
    ] {
        let project = dir.path().join(name);
        project_repo(&project, origin);
        let share = hub
            .share_repo(&request_in(&project), &BootReporter::none())
            .await
            .expect("share");
        assert!(share.repo.is_none() && share.secret.is_none(), "{name}");
        let claim = CloudHub::git_claim(Some("aura/scout-sc01"), None, None, None).expect("claim");
        assert_eq!((claim.remote_url, claim.env), (None, None));
    }
    assert_eq!(CloudHub::git_claim(None, Some(&repo), None, None), None);
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
    };
    hub.placements
        .lock()
        .agents
        .insert(agent.to_hex(), placed.clone());
    placed
}

#[tokio::test]
async fn the_agents_commits_come_back_on_its_branch_once_each() {
    let (hub, fake, dir) = hub();
    let project = dir.path().join("project");
    project_repo(&project, None);
    let root = dir.path().join("vm");
    std::fs::create_dir_all(&root).expect("mkdir");
    let machine = root.join("workspace");
    test_git(
        dir.path(),
        &[
            "clone",
            "-q",
            &project.display().to_string(),
            &machine.display().to_string(),
        ],
    );
    test_git(&machine, &["update-ref", workspace::HUB_HEAD_REF, "HEAD"]);
    let branch = "aura/scout-sc01";
    test_git(&machine, &["checkout", "-q", "-b", branch]);
    test_commit(&machine, "b.txt", "agent work");
    let tip = test_commit(&machine, "c.txt", "more agent work");
    run_scripts_in(&fake, &root, &machine);
    let agent = AgentId::new([1; 32]);
    let hex = agent.to_hex();
    let placed = adopt_machine(&hub, &fake, agent, &project, Some(branch)).await;

    assert_eq!(
        hub.fetch_commits(&hex, &placed).await,
        CommitsOutcome::Fetched {
            branch: branch.into(),
            commits: 2
        }
    );
    assert_eq!(
        test_git(&project, &["rev-parse", &format!("refs/heads/{branch}")]),
        tip
    );
    // The project's own branch and files are left as they were.
    assert_eq!(test_git(&project, &["branch", "--show-current"]), "main");
    assert!(!project.join("b.txt").exists());

    // What came is remembered: nothing new comes again.
    let placed = hub.placed_agent(agent).expect("placed");
    assert_eq!(placed.fetched_tip.as_deref(), Some(tip.as_str()));
    assert_eq!(hub.fetch_commits(&hex, &placed).await, CommitsOutcome::None);
    test_commit(&machine, "d.txt", "last");
    assert_eq!(
        hub.fetch_commits(&hex, &placed).await,
        CommitsOutcome::Fetched {
            branch: branch.into(),
            commits: 1
        }
    );

    // A branch checked out here is not moved under the user.
    test_git(&project, &["checkout", "-q", branch]);
    test_commit(&machine, "e.txt", "after the checkout");
    let placed = hub.placed_agent(agent).expect("placed");
    let CommitsOutcome::Kept { reason, .. } = hub.fetch_commits(&hex, &placed).await else {
        panic!("fetched into the checked-out branch");
    };
    assert!(reason.contains("checked out here"), "{reason}");

    // A placement from before agent branches has none to fetch.
    let legacy = PlacedAgent {
        branch: None,
        ..placed
    };
    assert_eq!(hub.fetch_commits(&hex, &legacy).await, CommitsOutcome::None);
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

#[test]
fn mail_never_crosses_users() {
    let (hub, _fake, _dir) = hub();
    hub.set_current_owner(alice());
    let local = AgentId::new([1; 32]);
    hub.note_local(local, "local", "p1");
    let alice_cloud = AgentId::new([2; 32]);
    let bob_cloud = AgentId::new([3; 32]);
    place_cloud(&hub, alice_cloud, "aura-alice", &alice());
    place_cloud(&hub, bob_cloud, "aura-bob", &bob());

    // Bob's machine can't mail Alice's local agent or her cloud agent.
    let err = ack_error(hub.take_mail(&mail_to(bob_cloud, "aura-bob", local, 1)));
    assert!(err.is_some_and(|e| e.contains("another user")));
    let err = ack_error(hub.take_mail(&mail_to(bob_cloud, "aura-bob", alice_cloud, 2)));
    assert!(err.is_some_and(|e| e.contains("another user")));
    // Alice's machine reaches her local agent (no host here: refused
    // for that reason, not for ownership).
    let err = ack_error(hub.take_mail(&mail_to(alice_cloud, "aura-alice", local, 3)));
    assert!(err.is_some_and(|e| !e.contains("another user")));

    // Local agents act as Alice: Bob's cloud agent is unknown to them.
    let router = HubRouter(Arc::downgrade(&hub));
    assert_eq!(router.name_of(bob_cloud), None);
    assert!(router.name_of(alice_cloud).is_some());
    let err = router
        .route(RemoteMail {
            from: local,
            from_name: "local".into(),
            to: bob_cloud,
            kind: MessageKind::Request,
            thread: ThreadId("t1".into()),
            subject: "s".into(),
            text: "hi".into(),
        })
        .expect_err("cross-user");
    assert!(err.contains("another user"), "{err}");

    // Each node's directory holds only its owner's agents.
    let MeshFrame::Directory { peers } = hub.directory.frame_for("aura-bob", "p1") else {
        panic!("not a directory");
    };
    assert!(peers.is_empty(), "{peers:?}");
}

#[tokio::test]
async fn an_agent_that_was_never_placed_recalls_to_nothing() {
    let (hub, _fake, _dir) = hub();
    let agent = AgentId::new([5; 32]);
    assert!(!hub.is_placed(agent));
    assert_eq!(
        hub.recall(agent, "test", &BootReporter::none())
            .await
            .expect("recall"),
        PullOutcome::patch_only(PatchOutcome::Empty)
    );
    assert_eq!(hub.placement_of(agent), Placement::Local);
}

#[test]
fn patch_outcomes_describe_themselves() {
    assert_eq!(PatchOutcome::Empty.describe(), "no changes to pull");
    assert_eq!(
        PatchOutcome::Applied {
            files: vec!["a".into()]
        }
        .describe(),
        "pulled 1 changed file(s)"
    );
    assert_eq!(
        PullOutcome {
            patch: PatchOutcome::Empty,
            commits: CommitsOutcome::Fetched {
                branch: "aura/scout-a1b2".into(),
                commits: 2,
            },
        }
        .describe(),
        "no changes to pull; 2 commit(s) on branch aura/scout-a1b2"
    );
    assert_eq!(
        PullOutcome::patch_only(PatchOutcome::Empty).describe(),
        "no changes to pull"
    );
}

#[test]
fn cloud_turns_never_offer_computer_use() {
    let request = chat_request(AgentId::new([1; 32]), None, None);
    let json = serde_json::to_value(&request.agent_permissions).expect("json");
    let text = json.to_string();
    assert!(!text.contains("computer"), "{text}");
    assert!(
        text.contains("spawnAgent") || text.contains("spawn_agent"),
        "{text}"
    );
}
