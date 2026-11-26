//! What a machine reports and is asked: titles, names, the work under
//! way there, its files and plans.

use super::*;

#[tokio::test]
async fn the_machine_titles_the_chat_during_the_turn_and_records_the_call() {
    let p = placed(CloudConfig::defaults()).await;
    p.import().await;

    let ask = crate::cloud::hub::TurnAsk {
        prompt: "fix the build".into(),
        model: Some("mock-model".into()),
        title_model: Some("mock-model".into()),
        ..Default::default()
    };
    let mut events = p.hub.run_turn(p.agent, ask, CancellationToken::new());
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
    let copied = hooks::store(&p.hub)
        .scan_record(p.agent, 1, 1_000)
        .expect("scan");
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
    hooks::directory(&p.hub).place(
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

    assert_eq!(
        p.hub.placed_agent(p.agent).expect("placed").name,
        "Build fix"
    );
    let saved = std::fs::read_to_string(p.hub_dir.path().join("cloud/placements.json"))
        .expect("placements saved");
    assert!(saved.contains("Build fix"), "{saved}");
    assert_eq!(
        hooks::directory(&p.hub)
            .holder(&hex)
            .map(|holder| holder.name),
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
    let streamed = hooks::streaming_turn(&p.hub, &hex, "run-1".into());
    hooks::note_processes(
        &p.hub,
        &hex,
        vec![process("turn:run-1:1", &hex, "running", None)],
    );
    assert!(
        p.hub.machine_processes().is_empty(),
        "shown as its own turn"
    );

    drop(streamed);
    hooks::note_processes(
        &p.hub,
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

/// What `hex`'s machine reports: its own turn and another agent's, the
/// `helper` it spawned with a `grandchild`, an agent `known` here, and
/// one spawned by an agent that isn't placed.
fn reported_work(hex: &str, helper: &str, grandchild: &str, known: AgentId) -> Vec<MeshProcess> {
    vec![
        process("turn:r1", hex, "running", None),
        process("turn:r2", &AgentId::new([5; 32]).to_hex(), "running", None),
        process("agent:h", helper, "idle", Some(hex)),
        process("agent:g", grandchild, "running", Some(helper)),
        process("agent:k", &known.to_hex(), "running", Some(hex)),
        process(
            "agent:o",
            &AgentId::new([4; 32]).to_hex(),
            "running",
            Some("ff"),
        ),
    ]
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
        hooks::store(&p.hub).as_ref(),
        1,
        reply(known, "an agent of this computer"),
    );

    hooks::note_processes(
        &p.hub,
        &hex,
        reported_work(&hex, &helper, &grandchild, known),
    );

    let shown = p.hub.machine_processes();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, hex);
    let ids: Vec<&str> = shown[0].1.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, ["turn:r1", "agent:h", "agent:g"]);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    hooks::sleep_idle_machines(&p.hub).await;
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Running,
        "kept awake while its work runs"
    );

    hooks::note_processes(
        &p.hub,
        &hex,
        vec![process("agent:h", &helper, "idle", Some(&hex))],
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    hooks::sleep_idle_machines(&p.hub).await;
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Hibernated
    );

    hooks::forget_processes(&p.hub, &hex);
    assert!(p.hub.machine_processes().is_empty());
}

/// The machine's workspace: `src/main.rs`, two plans and a note
/// beside them.
fn write_workspace(p: &Placed) {
    let ws = p.node_dir.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).expect("src");
    std::fs::write(ws.join("src/main.rs"), "fn main() {}\n").expect("main");
    std::fs::create_dir_all(ws.join(".aura/plans")).expect("plans");
    for (name, title) in [("a_00000001", "First"), ("b_00000002", "Second")] {
        std::fs::write(
            ws.join(format!(".aura/plans/{name}.plan.md")),
            format!("---\nname: {title}\ntodos: []\n---\n"),
        )
        .expect("plan");
    }
    std::fs::write(ws.join(".aura/plans/notes.md"), "not a plan").expect("notes");
}

#[tokio::test]
async fn an_awake_machine_s_files_and_plans_are_read() {
    let p = placed(CloudConfig::defaults()).await;
    write_workspace(&p);
    let root = Path::new(WORKSPACE_DIR);

    let listed = p.hub.list_files(p.agent, root).await.expect("listed");
    let names: Vec<(&str, bool)> = listed
        .iter()
        .map(|entry| (entry.name.as_str(), entry.is_dir))
        .collect();
    assert_eq!(names, [("src", true)], "dot entries are left out");
    let src = p
        .hub
        .list_files(p.agent, &root.join("src"))
        .await
        .expect("listed");
    assert_eq!(src.len(), 1);
    assert_eq!(src[0].name, "main.rs");
    assert!(p
        .hub
        .list_files(p.agent, &root.join("gone"))
        .await
        .expect("listed")
        .is_empty());
    assert_eq!(
        p.hub
            .read_file(p.agent, &root.join("src/main.rs"))
            .await
            .expect("read"),
        "fn main() {}\n"
    );
    assert!(p
        .hub
        .read_file(p.agent, Path::new("/workspace/../db"))
        .await
        .is_err());

    let plans = p
        .hub
        .machine_plans(p.agent, Some(".aura/plans/b_00000002.plan.md"))
        .await
        .expect("plans");
    let paths: Vec<&str> = plans.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            ".aura/plans/b_00000002.plan.md",
            ".aura/plans/a_00000001.plan.md"
        ]
    );
    assert!(plans[0].1.contains("name: Second"));
}

#[tokio::test]
async fn a_sleeping_machine_s_files_are_left_asleep() {
    let p = placed(CloudConfig::defaults()).await;
    write_workspace(&p);
    let root = Path::new(WORKSPACE_DIR);

    p.hub.sleep(p.agent).await.expect("slept");
    assert!(p.hub.list_files(p.agent, root).await.is_err());
    assert!(p.hub.machine_plans(p.agent, None).await.is_err());
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Hibernated,
        "looking never wakes it"
    );
}
