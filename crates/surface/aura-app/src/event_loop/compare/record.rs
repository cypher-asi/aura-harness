//! Each comparison's latest state, read back from its own record, and
//! how a comparison that stopped without recording its end reads.

use crate::event_loop::subagents::lock;
use crate::event_loop::LoopState;
use aura_core_types::{AgentId, ComparePayload, CompareStatus};
use aura_store_db::ReadStore;

/// Entries scanned back from the head of a comparison's record for its
/// latest state.
const SCAN_LIMIT: usize = 2_000;

/// The latest state of the comparison held by `host`, if recorded.
pub(super) fn latest_payload(store: &dyn ReadStore, host: AgentId) -> Option<ComparePayload> {
    let head = store.get_head_seq(host).ok()?;
    store
        .scan_record_descending(host, head, SCAN_LIMIT)
        .ok()?
        .iter()
        .find_map(|entry| entry.tx.compare_payload())
}

/// Mark `payload`, recorded as running when nothing runs it any more, as
/// stopped, saying why.
pub(super) fn interrupted(payload: &mut ComparePayload, why: &str) {
    payload.status = CompareStatus::Cancelled;
    payload.notes.push(format!(
        "It stopped before it ended ({why}); the runs that ended are kept"
    ));
}

/// Every comparison, oldest first, with its latest state. One whose
/// record says it runs while this process runs none of it is stopped: Aura
/// quit, or its task failed, before it recorded how it ended.
pub(crate) fn comparisons(state: &LoopState<'_>) -> Vec<(AgentId, String, Option<ComparePayload>)> {
    let hosts: Vec<(AgentId, String)> = lock(&state.hub.registry)
        .compares()
        .iter()
        .map(|agent| (agent.agent_id(), agent.name.clone()))
        .collect();
    let store = state.hub.factory.store.as_ref();
    let running = state.compare.running_host();
    hosts
        .into_iter()
        .map(|(id, name)| {
            let mut payload = latest_payload(store, id);
            if let Some(payload) = payload
                .as_mut()
                .filter(|p| p.status == CompareStatus::Running && running != Some(id))
            {
                interrupted(payload, "Aura quit or its task failed");
            }
            (id, name, payload)
        })
        .collect()
}
