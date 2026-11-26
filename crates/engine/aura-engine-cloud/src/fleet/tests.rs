
use super::*;
use crate::boot::{BootEvent, StageState};
use crate::fake::FakeProvider;
use aura_core_types::VmUsage;

fn settings(idle: Duration) -> FleetSettings {
    FleetSettings {
        install: "inst-1".into(),
        size: MachineSize::S4x16,
        isolated: true,
        idle_hibernate: idle,
        idle_standby: Duration::ZERO,
    }
}

fn fleet(idle: Duration) -> (Arc<VmFleet>, FakeProvider, Arc<MemoryLedger>) {
    let fake = FakeProvider::new();
    fake.on_exec(Arc::new(|machine, command, stdin| {
        assert_eq!(command, CLAIM_COMMAND);
        let claim: NodeClaim =
            serde_json::from_slice(stdin.unwrap_or_default()).expect("claim json");
        assert_eq!(claim.node_id, machine);
        assert_eq!(claim.node_token.len(), 64);
        let answer = NodeClaimed {
            node_id: claim.node_id,
            port: 8080,
            sealed: claim.seal.len(),
        };
        crate::ExecOutput {
            exit_code: 0,
            stdout: serde_json::to_vec(&answer).expect("json"),
            stderr: Vec::new(),
        }
    }));
    let ledger = Arc::new(MemoryLedger::default());
    let fleet = Arc::new(VmFleet::new(
        Arc::new(fake.clone()),
        ledger.clone(),
        settings(idle),
    ));
    (fleet, fake, ledger)
}

fn request(agent: &str, origin: MachineOrigin) -> ProvisionRequest {
    ProvisionRequest {
        agent: agent.into(),
        agent_zns: format!("0://terminal/{agent}"),
        code: Some("RE36".into()),
        project: "p1".into(),
        name: format!("aura-re36-{agent}"),
        origin,
        seal: Vec::new(),
        owner: ALICE.into(),
        secrets: Vec::new(),
        plans: Vec::new(),
        repo_secret: None,
        git: None,
    }
}

const ALICE: &str = "a11ce000";
const BOB: &str = "b0b00000";

#[tokio::test]
async fn a_machine_lists_one_repo_secret_and_its_claim_names_the_branch() {
    let (fleet, fake, _) = fleet(Duration::from_secs(300));
    let claims = Arc::new(Mutex::new(Vec::<NodeClaim>::new()));
    let seen = Arc::clone(&claims);
    fake.on_exec(Arc::new(move |_, _, stdin| {
        if let Some(claim) = stdin.and_then(|s| serde_json::from_slice(s).ok()) {
            seen.lock().push(claim);
        }
        crate::provider::ExecOutput::default()
    }));
    let plan = crate::names::plan_secret(ALICE, "inst-1", aura_core_auth::ProviderKind::Claude)
        .expect("name");
    let repo = crate::names::repo_secret("inst-1", "acme/app");
    for name in [&plan, &repo] {
        fake.set_secret(&crate::provider::HostSecret {
            name: name.clone(),
            value: zeroize::Zeroizing::new("unlinked".into()),
            domains: vec!["github.com".into()],
            replace_in: vec!["headers".into()],
        })
        .await
        .expect("secret");
    }
    let mut aa = request("aa", MachineOrigin::Fresh);
    aa.secrets = vec![plan.clone()];
    aa.repo_secret = Some(repo.clone());
    aa.git = Some(GitClaim {
        branch: "aura/reviewer-re36".into(),
        remote_url: Some("https://github.com/acme/app.git".into()),
        env: Some(repo.clone()),
        placeholder: None,
        author_name: Some("aura-agents[bot]".into()),
        author_email: None,
    });
    fleet.provision(aa).await.expect("provision");
    assert_eq!(
        fake.machine_secrets("aura-re36-aa"),
        vec![plan, repo.clone()]
    );
    let git = claims.lock()[0].git.clone().expect("git claim");
    assert_eq!(git.branch, "aura/reviewer-re36");
    assert_eq!(git.env.as_deref(), Some(repo.as_str()));

    // A plan secret passed as the repository secret is refused.
    let mut bb = request("bb", MachineOrigin::Fresh);
    bb.repo_secret = Some(
        crate::names::plan_secret(BOB, "inst-1", aura_core_auth::ProviderKind::Claude)
            .expect("name"),
    );
    let err = fleet.provision(bb).await.expect_err("not a repo secret");
    assert!(err.to_string().contains("repository secret"), "{err}");
}

#[tokio::test]
async fn machines_carry_their_owner_and_only_their_owners_secrets() {
    let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
    let secret = |owner: &str| {
        crate::names::plan_secret(owner, "inst-1", aura_core_auth::ProviderKind::Claude)
            .expect("name")
    };
    for owner in [ALICE, BOB] {
        fake.set_secret(&crate::provider::HostSecret {
            name: secret(owner),
            value: zeroize::Zeroizing::new("unlinked".into()),
            domains: vec!["api.anthropic.com".into()],
            replace_in: Vec::new(),
        })
        .await
        .expect("secret");
    }
    let mut aa = request("aa", MachineOrigin::Fresh);
    aa.secrets = vec![secret(ALICE)];
    let view = fleet.provision(aa).await.expect("aa");
    assert_eq!(view.owner, ALICE);
    assert_eq!(
        view.machine.labels.get(LABEL_OWNER).map(String::as_str),
        Some(ALICE)
    );
    assert_eq!(fake.machine_secrets("aura-re36-aa"), vec![secret(ALICE)]);
    assert!(ledger
        .rows()
        .iter()
        .all(|(_, row)| row.owner.as_deref() == Some(ALICE)));

    // Bob's secret is refused for Alice's machine.
    let mut cc = request("cc", MachineOrigin::Fresh);
    cc.secrets = vec![secret(BOB)];
    let err = fleet.provision(cc).await.expect_err("foreign secret");
    assert!(err.to_string().contains("not its owner's plan"), "{err}");

    // Bob can't fork Alice's machine.
    let mut bb = request("bb", MachineOrigin::Fork("aura-re36-aa".into()));
    bb.owner = BOB.into();
    let err = fleet.provision(bb).await.expect_err("cross-owner fork");
    assert!(err.to_string().contains("another user's machine"), "{err}");
    // Alice can, and the fork keeps her secrets.
    let dd = request("dd", MachineOrigin::Fork("aura-re36-aa".into()));
    fleet.provision(dd).await.expect("same-owner fork");
    assert_eq!(fake.machine_secrets("aura-re36-dd"), vec![secret(ALICE)]);

    assert_eq!(fleet.views_of(ALICE).len(), 2);
    assert!(fleet.views_of(BOB).is_empty());
    assert!(fleet.project_mesh_token(ALICE, "p1").is_some());
    assert!(
        fleet.project_mesh_token(BOB, "p1").is_none(),
        "mesh tokens are per owner"
    );
}

#[tokio::test]
async fn provisioning_labels_claims_and_records() {
    let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
    let view = fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("provision");
    assert_eq!(view.state, VmState::Ready);
    let machine = fake.machine("aura-re36-aa").expect("machine");
    assert_eq!(
        machine.labels.get(LABEL_AGENT).map(String::as_str),
        Some("aa")
    );
    assert_eq!(
        machine.labels.get(LABEL_INSTALL).map(String::as_str),
        Some("inst-1")
    );
    assert!(machine.isolated);
    assert!(fleet.node_token("aa").is_some());
    assert!(fake
        .calls()
        .iter()
        .any(|c| c == "SetBotProtection(aura-re36-aa, false)"));
    assert_eq!(
        ledger.events("0://cloud/vm/terminal/aa"),
        vec![VmEvent::Created, VmEvent::Claimed]
    );
}

fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<BootEvent>) -> Vec<BootEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

fn stage_trail(events: &[BootEvent]) -> Vec<(BootStage, &'static str)> {
    events
        .iter()
        .filter_map(|event| match event {
            BootEvent::Stage { stage, state } => Some((
                *stage,
                match state {
                    StageState::Started => "started",
                    StageState::Done { .. } => "done",
                    StageState::Skipped(_) => "skipped",
                    StageState::Failed(_) => "failed",
                },
            )),
            _ => None,
        })
        .collect()
}

fn fact<'a>(events: &'a [BootEvent], wanted: &str) -> Option<&'a str> {
    events.iter().rev().find_map(|event| match event {
        BootEvent::Fact { key, value } if key == wanted => Some(value.as_str()),
        _ => None,
    })
}

#[tokio::test]
async fn provisioning_shows_each_stage_and_the_machine() {
    let (fleet, _, _) = fleet(Duration::from_secs(300));
    let (boot, mut rx) = BootReporter::channel();
    fleet
        .provision_reported(request("aa", MachineOrigin::Fresh), &boot)
        .await
        .expect("provision");
    let events = drain(&mut rx);
    assert_eq!(
        stage_trail(&events),
        vec![
            (BootStage::Quota, "started"),
            (BootStage::Quota, "done"),
            (BootStage::Create, "started"),
            (BootStage::Create, "done"),
            (BootStage::Configure, "started"),
            (BootStage::Configure, "done"),
            (BootStage::Claim, "started"),
            (BootStage::Claim, "done"),
        ]
    );
    assert_eq!(fact(&events, "machine"), Some("aura-re36-aa"));
    assert_eq!(fact(&events, "network"), Some("isolated"));
    assert_eq!(fact(&events, "node"), Some("aura-re36-aa on port 8080"));
    assert!(fact(&events, "size").is_some_and(|size| size.contains("vCPU")));
    let token = fleet.node_token("aa").expect("token");
    assert!(
        !format!("{events:?}").contains(token.expose()),
        "the node token is never shown"
    );
}

#[tokio::test]
async fn provisioning_says_what_each_slow_call_waits_on() {
    let (fleet, _, _) = fleet(Duration::from_secs(300));
    let (boot, mut rx) = BootReporter::channel();
    fleet
        .provision_reported(request("aa", MachineOrigin::Fresh), &boot)
        .await
        .expect("provision");
    let waits: Vec<(BootStage, Option<String>, Option<Duration>)> = drain(&mut rx)
        .into_iter()
        .filter_map(|event| match event {
            BootEvent::Waiting { stage, what, limit } => Some((stage, what, limit)),
            _ => None,
        })
        .collect();
    let set = |stage: BootStage| {
        waits
            .iter()
            .find(|(s, what, _)| *s == stage && what.is_some())
            .cloned()
    };
    let create = set(BootStage::Create).expect("create waits");
    assert!(create.1.is_some_and(|what| what.contains("aura-re36-aa")));
    assert_eq!(create.2, Some(CREATE_TIMEOUT));
    let claim = set(BootStage::Claim).expect("claim waits");
    assert!(claim.1.is_some_and(|what| what.contains(CLAIM_COMMAND)));
    assert_eq!(claim.2, Some(CLAIM_TIMEOUT));
    assert!(set(BootStage::Quota).is_some());
    assert!(set(BootStage::Configure).is_some());
    // Every wait is cleared: as many clears as sets, and the last
    // word on each stage is a clear.
    let sets = waits.iter().filter(|(_, what, _)| what.is_some()).count();
    let clears = waits.iter().filter(|(_, what, _)| what.is_none()).count();
    assert_eq!(sets, clears);
    for stage in [
        BootStage::Quota,
        BootStage::Create,
        BootStage::Configure,
        BootStage::Claim,
    ] {
        let last = waits.iter().rev().find(|(s, _, _)| *s == stage);
        assert!(last.is_some_and(|(_, what, _)| what.is_none()), "{stage:?}");
    }
}

#[tokio::test]
async fn a_failed_claim_shows_as_a_failed_stage() {
    let (fleet, fake, _) = fleet(Duration::from_secs(300));
    fake.on_exec(Arc::new(|_, _, _| crate::ExecOutput {
        exit_code: 127,
        stdout: Vec::new(),
        stderr: b"aura: command not found".to_vec(),
    }));
    let (boot, mut rx) = BootReporter::channel();
    fleet
        .provision_reported(request("aa", MachineOrigin::Fresh), &boot)
        .await
        .expect_err("claim fails");
    let trail = stage_trail(&drain(&mut rx));
    assert_eq!(trail.last(), Some(&(BootStage::Claim, "failed")));
}

#[tokio::test]
async fn waking_shows_only_when_the_machine_sleeps() {
    let (fleet, _, _) = fleet(Duration::from_secs(300));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("provision");
    let (boot, mut rx) = BootReporter::channel();
    fleet
        .ensure_awake_reported("aa", &boot)
        .await
        .expect("awake");
    assert!(drain(&mut rx).is_empty(), "an awake machine shows nothing");
    fleet.set_state("aa", VmState::Hibernated);
    fleet
        .ensure_awake_reported("aa", &boot)
        .await
        .expect("wake");
    let events = drain(&mut rx);
    assert_eq!(
        stage_trail(&events),
        vec![(BootStage::Wake, "started"), (BootStage::Wake, "done")]
    );
    assert_eq!(fact(&events, "woke from"), Some("asleep"));
}

#[tokio::test]
async fn a_failed_claim_destroys_the_machine() {
    let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
    fake.on_exec(Arc::new(|_, _, _| crate::ExecOutput {
        exit_code: 127,
        stdout: Vec::new(),
        stderr: b"aura: command not found".to_vec(),
    }));
    let err = fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect_err("claim fails");
    assert!(err.to_string().contains("command not found"), "{err}");
    assert!(fake.machine("aura-re36-aa").is_none());
    assert!(!fleet.has("aa"));
    assert_eq!(
        ledger.events("0://cloud/vm/terminal/aa"),
        vec![VmEvent::Created, VmEvent::Error]
    );
}

#[tokio::test]
async fn the_machine_limit_is_checked_first() {
    let (fleet, fake, _) = fleet(Duration::from_secs(300));
    fake.set_machine_limit(1);
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("first fits");
    let err = fleet
        .provision(request("bb", MachineOrigin::Fresh))
        .await
        .expect_err("full");
    assert!(matches!(err, CloudError::MachineLimit { count: 1, max: 1 }));
}

#[tokio::test]
async fn idle_machines_hibernate_and_busy_ones_do_not() {
    let (fleet, fake, ledger) = fleet(Duration::from_millis(1));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("aa");
    fleet
        .provision(request("bb", MachineOrigin::Fresh))
        .await
        .expect("bb");
    let guard = fleet.begin_work("bb");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let slept = fleet.tick().await;
    assert_eq!(slept, vec!["aa".to_string()]);
    assert_eq!(
        fake.machine("aura-re36-aa").expect("aa").status,
        MachineStatus::Hibernated
    );
    assert_eq!(
        fake.machine("aura-re36-bb").expect("bb").status,
        MachineStatus::Running
    );
    assert_eq!(fleet.view("aa").expect("aa").state, VmState::Hibernated);
    assert!(ledger
        .events("0://cloud/vm/terminal/aa")
        .contains(&VmEvent::Hibernated));
    drop(guard);
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(fleet.tick().await, vec!["bb".to_string()]);
}

#[tokio::test]
async fn work_for_a_sleeping_machine_wakes_it_first() {
    let (fleet, fake, ledger) = fleet(Duration::from_millis(1));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("aa");
    tokio::time::sleep(Duration::from_millis(5)).await;
    fleet.tick().await;
    let view = fleet.ensure_awake("aa").await.expect("wake");
    assert_eq!(view.state, VmState::Ready);
    assert_eq!(
        fake.machine("aura-re36-aa").expect("aa").status,
        MachineStatus::Running
    );
    assert_eq!(fake.calls_matching("Wake(").len(), 1);
    assert!(ledger
        .events("0://cloud/vm/terminal/aa")
        .ends_with(&[VmEvent::Hibernated, VmEvent::Woke]));
}

#[tokio::test]
async fn a_machine_woken_by_a_peer_is_noticed() {
    let (fleet, fake, _) = fleet(Duration::from_millis(1));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("aa");
    tokio::time::sleep(Duration::from_millis(5)).await;
    fleet.tick().await;
    fake.set_status("aura-re36-aa", MachineStatus::Running);
    assert_eq!(fleet.poll().await, vec!["aa".to_string()]);
    assert_eq!(fleet.view("aa").expect("aa").state, VmState::Ready);
}

#[tokio::test]
async fn destroying_records_the_last_reading_then_the_final_one() {
    let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
    let view = fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("aa");
    fake.set_usage(
        "aura-re36-aa",
        VmUsage {
            cost_micro: 410_000,
            ..VmUsage::default()
        },
    );
    fleet.record_usage().await;
    assert_eq!(
        fleet
            .view("aa")
            .and_then(|v| v.usage)
            .map(|u| u.usage.cost_micro),
        Some(410_000)
    );
    let id = fleet
        .destroy("aa", "archived")
        .await
        .expect("destroy")
        .expect("had one");
    assert!(fake.machine("aura-re36-aa").is_none());
    fake.set_usage(
        "aura-re36-aa",
        VmUsage {
            cost_micro: 450_000,
            complete: true,
            ..VmUsage::default()
        },
    );
    fleet
        .settle_usage(
            &id,
            &view.machine.name,
            &view.ledger,
            2,
            Duration::from_millis(1),
        )
        .await;
    let rows = ledger.rows();
    let last = &rows.last().expect("rows").1;
    assert_eq!(last.event, VmEvent::Usage);
    assert_eq!(
        last.usage.map(|u| (u.cost_micro, u.complete)),
        Some((450_000, true))
    );
    assert!(ledger.events(&view.ledger).contains(&VmEvent::Destroyed));
}

#[tokio::test]
async fn usage_refused_for_a_member_key_falls_back_quietly() {
    let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("aa");
    fake.deny_usage();
    fleet.record_usage().await;
    assert!(fleet.usage_denied());
    assert!(!ledger
        .events("0://cloud/vm/terminal/aa")
        .contains(&VmEvent::Usage));
}

#[tokio::test]
async fn orphans_are_swept_and_kept_agents_are_not() {
    let (fleet, fake, _) = fleet(Duration::from_secs(300));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("aa");
    fleet
        .provision(request("bb", MachineOrigin::Fresh))
        .await
        .expect("bb");
    let keep: HashSet<String> = ["aa".to_string()].into_iter().collect();
    let destroyed = fleet.sweep_orphans(&keep).await.expect("sweep");
    assert_eq!(destroyed, vec!["aura-re36-bb".to_string()]);
    assert!(fake.machine("aura-re36-aa").is_some());
}

#[tokio::test]
async fn forks_copy_the_parent_and_seal_its_agents() {
    let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("aa");
    let mut child = request("bb", MachineOrigin::Fork("aura-re36-aa".into()));
    child.seal = vec!["aa".into()];
    let view = fleet.provision(child).await.expect("fork");
    assert_eq!(
        view.machine.source,
        Some(VmSource::Fork {
            parent: "aura-re36-aa".into()
        })
    );
    assert_eq!(
        ledger.events("0://cloud/vm/terminal/bb")[0],
        VmEvent::Forked
    );
    assert!(fake
        .calls()
        .iter()
        .any(|c| c.starts_with("Create(aura-re36-bb, Fork(")));
    // Each node gets its own token; the project shares one mesh token.
    assert_ne!(
        fleet.node_token("aa").map(|t| t.expose().to_string()),
        fleet.node_token("bb").map(|t| t.expose().to_string())
    );
    assert!(fleet.project_mesh_token(ALICE, "p1").is_some());
}

#[tokio::test]
async fn an_adopted_machine_is_claimed_again_on_first_use() {
    let (fleet, fake, _) = fleet(Duration::from_secs(300));
    fleet
        .provision(request("aa", MachineOrigin::Fresh))
        .await
        .expect("aa");
    // A new hub process: a new fleet over the same provider.
    let ledger = Arc::new(MemoryLedger::default());
    let restarted = Arc::new(VmFleet::new(
        Arc::new(fake.clone()),
        ledger,
        settings(Duration::from_secs(300)),
    ));
    restarted
        .adopt(
            "aa",
            "0://terminal/aa",
            "p1",
            "aura-re36-aa",
            Vec::new(),
            None,
        )
        .await
        .expect("adopt");
    assert!(restarted.node_token("aa").is_none());
    restarted.ensure_awake("aa").await.expect("claim");
    assert!(restarted.node_token("aa").is_some());
}
