//! What the hub socket is told as it changes: the hosted agents' record
//! heads, and the turns and spawned agents under way here.

use super::*;

/// [`watch_heads`] on a task of its own, until it is aborted.
pub(super) fn spawn_head_watch(
    tx: mpsc::UnboundedSender<MeshFrame>,
    state: RouterState,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { watch_heads(&state, &tx).await })
}

/// [`watch_processes`] on a task of its own, until it is aborted.
pub(super) fn spawn_process_watch(
    tx: mpsc::UnboundedSender<MeshFrame>,
    state: RouterState,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { watch_processes(&state, &tx).await })
}

/// Tell the hub on `tx` whenever a hosted agent's record grows, so it
/// copies the new entries while the machine is awake. The first look
/// reports every head, which covers what was written while no socket
/// was open.
pub(super) async fn watch_heads(state: &RouterState, tx: &mpsc::UnboundedSender<MeshFrame>) {
    let mut told: HashMap<AgentId, u64> = HashMap::new();
    loop {
        for agent in state.cloud.hosted() {
            let Ok(head) = state.store.get_head_seq(agent) else {
                continue;
            };
            if told.insert(agent, head) == Some(head) {
                continue;
            }
            let hint = MeshFrame::RecordCommitted {
                agent_id: agent.to_hex(),
                head_seq: head,
            };
            if tx.send(hint).is_err() {
                return;
            }
        }
        tokio::time::sleep(HEAD_WATCH_EVERY).await;
    }
}

/// Tell the hub on `tx` what its agents have under way here whenever
/// that changes: their turns, and the agents they spawned. The first
/// look is always sent, so the hub drops what an earlier socket said.
async fn watch_processes(state: &RouterState, tx: &mpsc::UnboundedSender<MeshFrame>) {
    let mut first_seen: HashMap<String, u64> = HashMap::new();
    let mut told: Option<Vec<MeshProcess>> = None;
    loop {
        let rows = node_processes(state, &mut first_seen);
        if told.as_ref() != Some(&rows) {
            let frame = MeshFrame::Processes {
                processes: rows.clone(),
            };
            if tx.send(frame).is_err() {
                return;
            }
            told = Some(rows);
        }
        tokio::time::sleep(PROCESS_WATCH_EVERY).await;
    }
}

/// `GET /v1/cloud/processes`: what [`node_processes`] finds now, the
/// node's own answer to whether a turn or a spawned agent runs here,
/// whoever started it. The hub asks before it restarts the node.
pub(in crate::gateway) async fn processes_handler(
    State(state): State<RouterState>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    Ok(Json(node_processes(&state, &mut HashMap::new())))
}

/// The hosted agents' turns under way and the agents they spawned, in a
/// stable order. `first_seen` dates the spawned agents' rows.
pub(super) fn node_processes(
    state: &RouterState,
    first_seen: &mut HashMap<String, u64>,
) -> Vec<MeshProcess> {
    let hosted = state.cloud.hosted();
    let mut rows: Vec<MeshProcess> = state
        .chat_runs
        .iter()
        .filter_map(|run| {
            let agent = run.value().agent.filter(|agent| hosted.contains(agent))?;
            let turn = run.value().activity.current()?;
            // A run takes turn after turn: each is a row of its own.
            Some(MeshProcess {
                id: format!("turn:{}:{}", run.key(), turn.started_ms),
                agent_id: agent.to_hex(),
                label: turn.label,
                status: "running".into(),
                started_ms: turn.started_ms,
                kind: None,
                run_id: Some(run.key().clone()),
                parent: None,
                name: None,
            })
        })
        .collect();
    let now = aura_agent::checkpoint::now_ms();
    for root in hosted {
        let tree = state.agent_host.tree(root);
        rows.extend(
            tree.iter()
                .filter_map(|info| spawned_row(root, info, first_seen, now)),
        );
    }
    first_seen.retain(|id, _| rows.iter().any(|row| row.id == *id));
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

/// The row of `info`, an agent in `root`'s tree, unless it is `root`.
pub(super) fn spawned_row(
    root: AgentId,
    info: &AgentInfo,
    first_seen: &mut HashMap<String, u64>,
    now: u64,
) -> Option<MeshProcess> {
    if matches!(info.state, AgentState::Root) {
        return None;
    }
    let id = format!("agent:{}", info.agent_id.to_hex());
    let started_ms = *first_seen.entry(id.clone()).or_insert(now);
    Some(MeshProcess {
        id,
        agent_id: info.agent_id.to_hex(),
        label: format!("{} ({})", info.name, info.role),
        status: info.state.as_str().into(),
        started_ms,
        kind: Some(MESH_PROCESS_AGENT.into()),
        run_id: None,
        parent: Some(info.parent.unwrap_or(root).to_hex()),
        name: Some(info.name.clone()),
    })
}
