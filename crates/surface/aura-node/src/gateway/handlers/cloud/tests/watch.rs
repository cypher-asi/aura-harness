//! What the hub socket is told as it changes: record hints, the turns
//! under way and the spawned agents.

use super::super::watch::{node_processes, spawned_row, watch_heads};
use super::support::{claim, cloud_state, import};
use crate::gateway::RouterState;
use aura_core_protocol::cloud::MeshFrame;
use aura_core_types::AgentId;
use aura_engine::agents::{AgentInfo, AgentState};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The next record hint the watcher sends within five seconds.
async fn next_hint(rx: &mut mpsc::UnboundedReceiver<MeshFrame>) -> Option<(String, u64)> {
    match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        Ok(Some(MeshFrame::RecordCommitted { agent_id, head_seq })) => Some((agent_id, head_seq)),
        _ => None,
    }
}

#[tokio::test(start_paused = true)]
async fn the_hub_hears_when_a_hosted_record_grows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let agent = AgentId::new([7; 32]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let watcher = {
        let state = state.clone();
        tokio::spawn(async move { watch_heads(&state, &tx).await })
    };
    assert_eq!(next_hint(&mut rx).await, None, "nothing hosted yet");

    import(&state, agent, 2).await;
    assert_eq!(next_hint(&mut rx).await, Some((agent.to_hex(), 2)));
    assert_eq!(next_hint(&mut rx).await, None, "said once per head");
    import(&state, agent, 3).await;
    assert_eq!(next_hint(&mut rx).await, Some((agent.to_hex(), 3)));

    state.cloud.seal(agent);
    assert!(
        state.cloud.hosted().is_empty(),
        "a sealed agent is not watched"
    );
    watcher.abort();
}

pub(super) fn run_of(
    state: &RouterState,
    run_id: &str,
    agent: AgentId,
) -> Arc<crate::gateway::session::chat_run::ChatRunHandle> {
    use crate::gateway::session::chat_run::{register_run, ChatEventChannel, RunRegistration};
    let (commands, _commands_rx) = mpsc::channel(1);
    register_run(
        &state.chat_runs,
        run_id.into(),
        RunRegistration {
            commands,
            events: ChatEventChannel::new(),
            attach_count: Arc::default(),
            shutdown: tokio_util::sync::CancellationToken::new(),
            linkage: None,
            agent: Some(agent),
            activity: Arc::default(),
        },
    )
}

#[tokio::test]
async fn the_hub_hears_of_turns_only_while_they_run_and_only_of_hosted_agents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let hosted = AgentId::new([1; 32]);
    state.cloud.host(hosted);
    let busy = run_of(&state, "r1", hosted);
    run_of(&state, "r2", AgentId::new([2; 32]))
        .activity
        .begin("Not hosted here".into());
    let mut first_seen = HashMap::new();
    assert!(node_processes(&state, &mut first_seen).is_empty());

    busy.activity.begin("Fix the parser".into());
    let rows = node_processes(&state, &mut first_seen);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].id.starts_with("turn:r1:"), "{rows:?}");
    assert_eq!(rows[0].run_id.as_deref(), Some("r1"));
    assert_eq!(rows[0].agent_id, hosted.to_hex());
    assert_eq!(rows[0].label, "Fix the parser");
    assert!(rows[0].is_running() && !rows[0].is_agent());

    busy.activity.end();
    assert!(node_processes(&state, &mut first_seen).is_empty());
}

#[test]
fn a_spawned_agent_is_a_row_under_its_parent_dated_when_first_seen() {
    let root = AgentId::new([1; 32]);
    let child = AgentId::new([2; 32]);
    let info = |agent_id, parent, state| AgentInfo {
        agent_id,
        name: "scout".into(),
        role: "explorer".into(),
        parent,
        depth: 1,
        state,
        model: String::new(),
        turns: 0,
        queued: 0,
        last_reply: None,
    };
    let mut first_seen = HashMap::new();
    assert_eq!(
        spawned_row(
            root,
            &info(root, None, AgentState::Root),
            &mut first_seen,
            5
        ),
        None
    );
    let row = spawned_row(
        root,
        &info(child, None, AgentState::Idle),
        &mut first_seen,
        5,
    )
    .expect("a row");
    assert!(row.is_agent() && !row.is_running());
    assert_eq!(row.id, format!("agent:{}", child.to_hex()));
    assert_eq!(row.label, "scout (explorer)");
    assert_eq!(row.name.as_deref(), Some("scout"));
    assert_eq!(row.parent, Some(root.to_hex()));

    let again = spawned_row(
        root,
        &info(child, Some(root), AgentState::Running),
        &mut first_seen,
        9,
    )
    .expect("a row");
    assert!(again.is_running());
    assert_eq!(again.started_ms, 5);
}
