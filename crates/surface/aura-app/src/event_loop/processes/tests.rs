use super::*;
use aura_agent::ProcessManagerConfig;
use aura_core_types::Transaction;

fn turn(agent: AgentId, started_ms: u64) -> ProcessSummary {
    ProcessSummary {
        id: turn_id(agent),
        kind: ProcessKind::Turn,
        label: "fix the build".into(),
        status: ProcessStatus::Running,
        owner_agent_id: agent.to_hex(),
        jump_agent_id: agent.to_hex(),
        started_ms,
        killable: true,
        ..ProcessSummary::default()
    }
}

fn task(parent: AgentId, child: AgentId, started_ms: u64) -> ProcessSummary {
    ProcessSummary {
        id: task_id(child),
        kind: ProcessKind::Task,
        label: "Reviewer-1 (reviewer)".into(),
        status: ProcessStatus::Running,
        owner_agent_id: parent.to_hex(),
        jump_agent_id: child.to_hex(),
        started_ms,
        killable: true,
        ..ProcessSummary::default()
    }
}

fn ids(rows: &[ProcessSummary]) -> Vec<(String, usize)> {
    rows.iter().map(|r| (r.id.clone(), r.depth)).collect()
}

#[test]
fn tasks_nest_under_the_turn_that_spawned_them() {
    let table = ProcessTable::without_os_scan(None, None);
    let root = AgentId::generate();
    let (first, second) = (AgentId::generate(), AgentId::generate());
    table.name_agent(root, "Agent01");
    table.start(task(root, second, 30), None);
    table.start(turn(root, 10), None);
    table.start(task(root, first, 20), None);

    let rows = table.snapshot();
    assert_eq!(
        ids(&rows),
        vec![
            (turn_id(root), 0),
            (task_id(first), 1),
            (task_id(second), 1)
        ]
    );
    assert!(rows.iter().all(|r| r.owner_name == "Agent01"));
}

#[test]
fn killing_a_turn_cancels_it_and_keeps_it_as_killed() {
    let table = ProcessTable::without_os_scan(None, None);
    let agent = AgentId::generate();
    let cancel = CancellationToken::new();
    table.start(turn(agent, 1), Some(cancel.clone()));

    let killed = table.kill(&turn_id(agent)).unwrap();
    assert_eq!(killed.kind, ProcessKind::Turn);
    assert_eq!(killed.owner, agent.to_hex());
    assert!(cancel.is_cancelled());

    table.finish(&turn_id(agent), ProcessStatus::Failed);
    let rows = table.snapshot();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, ProcessStatus::Killed);
    assert!(rows[0].ended_ms.is_some());
    assert!(!rows[0].killable);
    assert!(table.kill(&turn_id(agent)).is_err());
}

#[test]
fn finished_rows_are_capped_newest_first() {
    let table = ProcessTable::without_os_scan(None, None);
    let agents: Vec<AgentId> = (0..MAX_FINISHED + 5).map(|_| AgentId::generate()).collect();
    for (i, agent) in agents.iter().enumerate() {
        table.start(turn(*agent, i as u64), None);
        table.finish(&turn_id(*agent), ProcessStatus::Exited);
    }
    let rows = table.snapshot();
    assert_eq!(rows.len(), MAX_FINISHED);
    assert_eq!(rows[0].id, turn_id(*agents.last().unwrap()));
}

#[test]
fn finished_rows_outlive_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("process_history.json");
    let agent = AgentId::generate();
    let table = ProcessTable::without_os_scan(None, None);
    table.keep_history(path.clone());
    table.start(turn(agent, 1), None);
    table.save();
    assert!(!path.exists(), "running rows are not history");
    table.finish(&turn_id(agent), ProcessStatus::Exited);
    table.save();

    let next = ProcessTable::without_os_scan(None, None);
    next.keep_history(path);
    let rows = next.snapshot();
    assert_eq!(ids(&rows), vec![(turn_id(agent), 0)]);
    assert_eq!(rows[0].status, ProcessStatus::Exited);
    assert!(!rows[0].killable);
}

#[tokio::test]
async fn background_commands_are_listed_killed_and_reported() {
    let (tx, mut completions) = mpsc::channel::<Transaction>(4);
    let manager = Arc::new(ProcessManager::new(tx, ProcessManagerConfig::default()));
    let table = ProcessTable::without_os_scan(None, Some(Arc::clone(&manager)));
    let agent = AgentId::generate();
    table.name_agent(agent, "Agent01");
    table.start(turn(agent, 0), None);

    let process_id = ProcessId::generate();
    let child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id();
    manager.register_with(
        agent,
        Hash::from_content(b"proposal"),
        ActionId::generate(),
        process_id,
        child,
        "sleep 30".into(),
        ProcessMeta {
            cwd: Some(PathBuf::from("/tmp")),
            log_path: Some(PathBuf::from("/tmp/x.log")),
            tool_use_id: Some("toolu_1".into()),
        },
    );

    let rows = table.snapshot();
    let row = rows
        .iter()
        .find(|r| r.kind == ProcessKind::Command)
        .unwrap();
    assert_eq!(
        row.depth, 1,
        "nests under the turn of the agent that started it"
    );
    assert_eq!(row.pid, Some(pid));
    assert_eq!(row.owner_name, "Agent01");
    assert_eq!(row.tool_use_id.as_deref(), Some("toolu_1"));
    assert_eq!(
        row.log_path.as_deref(),
        Some(std::path::Path::new("/tmp/x.log"))
    );
    assert_eq!(
        row.spawn_tx,
        Some(hex::encode(Hash::from_content(b"proposal").as_bytes()))
    );

    let id = row.id.clone();
    table.kill(&id).unwrap();
    let completion = completions.recv().await.unwrap();
    let result: aura_core_types::ActionResultPayload =
        serde_json::from_slice(&completion.payload).unwrap();
    table.command_finished(result.process_id, result.exit_code, result.success);

    let rows = table.snapshot();
    let row = rows.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row.status, ProcessStatus::Killed);
}

#[cfg(target_os = "linux")]
#[test]
fn tagged_os_processes_are_listed_under_their_parent_and_killable() {
    use aura_exec_tools::{AGENT_ID_VAR, HARNESS_PID_VAR};
    use std::os::unix::process::CommandExt;

    let table = ProcessTable::new(None, None);
    let agent = AgentId::generate();
    table.name_agent(agent, "Agent01");
    table.start(turn(agent, 0), None);
    let mut child = std::process::Command::new("sh")
        .args(["-c", "sleep 30 & wait"])
        .env(AGENT_ID_VAR, agent.to_hex())
        .env(HARNESS_PID_VAR, std::process::id().to_string())
        .process_group(0)
        .spawn()
        .unwrap();

    let mut rows = Vec::new();
    for _ in 0..50 {
        rows = table.snapshot();
        if rows
            .iter()
            .filter(|r| r.owner_agent_id == agent.to_hex())
            .count()
            == 3
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mine: Vec<&ProcessSummary> = rows
        .iter()
        .filter(|r| r.owner_agent_id == agent.to_hex())
        .collect();
    assert_eq!(mine.len(), 3, "turn, shell and sleep: {mine:#?}");
    assert_eq!(mine[0].kind, ProcessKind::Turn);
    assert_eq!(mine[1].pid, Some(child.id()));
    assert_eq!(mine[1].depth, 1);
    assert_eq!(mine[2].label, "sleep 30");
    assert_eq!(
        mine[2].depth, 2,
        "the sleep nests under the shell that forked it"
    );
    assert_eq!(mine[2].owner_name, "Agent01");

    let shell = mine[1].id.clone();
    table.kill(&shell).unwrap();
    child.wait().unwrap();
    let mut gone = false;
    for _ in 0..50 {
        let rows = table.snapshot();
        if rows
            .iter()
            .filter(|r| r.owner_agent_id == agent.to_hex())
            .all(|r| r.kind == ProcessKind::Turn || !r.status.is_live())
        {
            gone = rows
                .iter()
                .any(|r| r.id == shell && r.status == ProcessStatus::Killed);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(gone, "the killed shell is kept as killed");
}
