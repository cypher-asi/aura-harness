use super::*;

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
async fn a_new_machine_whose_node_is_older_than_the_hub_fails_its_claim_and_goes() {
    let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
    fake.on_exec(Arc::new(|_, _, stdin| claimed_by_an_older_node(stdin)));
    let (boot, mut rx) = BootReporter::channel();
    let err = fleet
        .provision_reported(request("aa", MachineOrigin::Fresh), &boot)
        .await
        .expect_err("outdated");

    assert!(
        matches!(
            &err,
            CloudError::OutdatedNode { machine, has: 0, needs: NODE_PROTOCOL }
                if machine == "aura-re36-aa"
        ),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("scripts/build-cloud-binary.sh"),
        "{err}"
    );
    let trail = stage_trail(&drain(&mut rx));
    assert_eq!(trail.last(), Some(&(BootStage::Claim, "failed")));
    assert!(fake.machine("aura-re36-aa").is_none());
    assert!(!fleet.has("aa"));
    assert_eq!(
        ledger.events("0://cloud/vm/terminal/aa"),
        vec![VmEvent::Created, VmEvent::Claimed, VmEvent::Error]
    );
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
