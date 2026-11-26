use super::*;

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
