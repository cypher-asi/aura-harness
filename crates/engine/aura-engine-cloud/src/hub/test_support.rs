//! Hooks into a hub's internals for tests outside this crate that drive
//! it against a real node served in-process (`aura-app`'s hub tests).

use std::collections::HashMap;
use std::sync::Arc;

use aura_core_protocol::cloud::MeshProcess;
use aura_core_types::AgentId;
use aura_store_db::Store;

use crate::fleet::MachineView;
use crate::mesh::Directory;
use crate::{BootReporter, CloudResult};

use super::node_client::NodeClient;
use super::{CloudHub, PlaceRequest, PlacedAgent};

/// The store the hub copies records into.
pub fn store(hub: &CloudHub) -> &Arc<dyn Store> {
    &hub.store
}

/// Who holds which agent, as the hub tells the mesh.
pub fn directory(hub: &CloudHub) -> &Directory {
    &hub.directory
}

/// Record `agent` (hex `hex`) as placed, as a finished placement does.
pub fn insert_placement(hub: &CloudHub, hex: String, agent: PlacedAgent) {
    hub.placements.lock().agents.insert(hex, agent);
    hub.save_placements();
}

/// Hand the machine's node the local record, as a placement does.
pub async fn import_record(
    hub: &CloudHub,
    request: &PlaceRequest,
    view: &MachineView,
    boot: &BootReporter,
) -> CloudResult<()> {
    hub.import_record(request, view, boot).await
}

/// A client of the node holding `hex`.
pub fn node(hub: &CloudHub, hex: &str) -> CloudResult<NodeClient> {
    hub.node(hex)
}

/// Open the socket to the node holding `hex`.
pub fn open_link(hub: &Arc<CloudHub>, hex: &str) {
    hub.open_link(hex);
}

/// Ask for a copy of `agent`'s record soon, as a node's hint does.
pub fn copy_soon(hub: &Arc<CloudHub>, agent: AgentId) {
    hub.copy_soon(agent);
}

/// The lock a copy of `agent`'s record holds while it runs.
pub fn copy_gate(hub: &CloudHub, agent: AgentId) -> Arc<tokio::sync::Mutex<()>> {
    Arc::clone(hub.mirrors.lock().entry(agent).or_default())
}

/// The copies running, by agent: `true` when one more is owed.
pub fn copies(hub: &CloudHub) -> HashMap<AgentId, bool> {
    hub.copies.lock().clone()
}

/// One pass of the idle timer.
pub async fn sleep_idle_machines(hub: &CloudHub) {
    hub.sleep_idle_machines().await;
}

/// What runs before `agent`'s machine sleeps.
pub async fn copy_before_sleep(hub: &CloudHub, agent: AgentId) {
    hub.copy_before_sleep(agent).await;
}

/// Whether the changes `hex`'s machine held when it slept are kept.
pub fn has_saved_patch(hub: &CloudHub, hex: &str) -> bool {
    hub.saved_patch(hex).is_some()
}

/// Mark the turn of run `run` as streamed from this hub until the
/// returned guard drops.
pub fn streaming_turn<'a>(hub: &'a CloudHub, hex: &str, run: String) -> impl Sized + 'a {
    hub.streaming_turn(hex, run)
}

/// What `hex`'s machine reports it has under way, as its node's
/// PROCESSES frame says.
pub fn note_processes(hub: &CloudHub, hex: &str, rows: Vec<MeshProcess>) {
    hub.note_processes(hex, rows);
}

/// Forget what `hex`'s machine reported, as a closed socket does.
pub fn forget_processes(hub: &CloudHub, hex: &str) {
    hub.forget_processes(hex);
}

/// The placed agents whose machines a starting hub copies, hex.
pub fn catch_up_due(hub: &CloudHub) -> Vec<String> {
    hub.catch_up_due()
}
