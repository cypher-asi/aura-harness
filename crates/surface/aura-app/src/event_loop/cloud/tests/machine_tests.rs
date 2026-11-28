use super::*;
use aura_core_protocol::cloud::MESH_PROCESS_AGENT;

fn row(id: &str, agent: &str, status: &str, parent: Option<&str>) -> MeshProcess {
    MeshProcess {
        id: id.into(),
        agent_id: agent.into(),
        label: id.into(),
        status: status.into(),
        started_ms: 5,
        kind: parent.map(|_| MESH_PROCESS_AGENT.into()),
        run_id: None,
        parent: parent.map(str::to_string),
        name: parent.map(|_| format!("name of {agent}")),
    }
}

#[test]
fn a_machine_s_work_becomes_rows_members_can_list() {
    let rows = machine_rows(
        "aa",
        &[
            row("turn:r1:5", "aa", "running", None),
            row("agent:bb", "bb", "running", Some("aa")),
            row("agent:cc", "cc", "idle", Some("bb")),
        ],
    );

    assert_eq!(rows[0].id, "machine:turn:r1:5");
    assert_eq!(rows[0].kind, ProcessKind::Turn);
    assert_eq!(rows[0].owner_agent_id, "aa");

    assert_eq!(rows[1].id, "agent:bb");
    assert_eq!(rows[1].kind, ProcessKind::Agent);
    assert_eq!(rows[1].owner_agent_id, "aa");
    assert!(rows[1].parent_chain.is_empty());
    assert_eq!(rows[1].status, ProcessStatus::Running);

    assert_eq!(rows[2].owner_agent_id, "bb");
    assert_eq!(rows[2].owner_name, "name of bb");
    assert_eq!(rows[2].parent_chain, ["aa"]);
    assert_eq!(rows[2].status, ProcessStatus::Idle);

    let jumps: Vec<&str> = rows.iter().map(|row| row.jump_agent_id.as_str()).collect();
    assert_eq!(
        jumps,
        ["aa", "bb", "cc"],
        "a spawned agent opens its own chat"
    );
    assert!(rows.iter().all(|row| !row.killable));
}
