//! Keeping a cloud machine awake while it works with no hub connected.
//!
//! boxd hibernates a machine nothing has connected to for a while (its
//! backstop, half an hour). While the hub is away (Aura closed or
//! crashed), a turn's long quiet tool run looks just like that. So a
//! node with work under way and no hub socket calls its own public URL
//! every [`PING_EVERY`], which boxd counts as a connection, until its hub
//! has been away for [`KEEP_AWAKE_FOR`]: past that the machine is left to
//! the backstop, so one no Aura comes back for stops costing.

use super::watch::node_processes;
use super::*;

/// How often a node with work and no hub calls itself.
const PING_EVERY: Duration = Duration::from_secs(5 * 60);
/// How long after its hub was last connected a node stops keeping its
/// machine awake.
const KEEP_AWAKE_FOR: Duration = Duration::from_secs(6 * 60 * 60);

/// Whether a node calls itself now: work is under way, and the hub has
/// been away (`hub_away`, zero while connected) but not for long.
pub(super) fn keeps_awake(working: bool, hub_away: Duration) -> bool {
    working && !hub_away.is_zero() && hub_away < KEEP_AWAKE_FOR
}

/// Call this machine's public URL every [`PING_EVERY`] while
/// [`keeps_awake`] says so, until the process ends.
pub(in crate::gateway) async fn keep_awake(state: RouterState) {
    let mut first_seen = HashMap::new();
    loop {
        tokio::time::sleep(PING_EVERY).await;
        let working = node_processes(&state, &mut first_seen)
            .iter()
            .any(|row| row.status == "running");
        if !keeps_awake(working, state.mesh.hub_away()) {
            continue;
        }
        let Some(url) = state.cloud.public_url() else {
            continue;
        };
        let health = format!("{}/health", url.trim_end_matches('/'));
        match state.mesh.http.get(&health).send().await {
            Ok(_) => {
                tracing::debug!("called this machine's public URL to keep it awake while it works");
            }
            Err(err) => tracing::debug!(error = %err, "could not call this machine's public URL"),
        }
    }
}
