//! When a cloud node keeps its machine awake on its own.

use super::super::keepalive::keeps_awake;
use super::support::{claim, cloud_state};
use std::time::Duration;

const MINUTE: Duration = Duration::from_secs(60);

#[test]
fn a_node_keeps_its_machine_awake_only_while_it_works_with_its_hub_away() {
    assert!(keeps_awake(true, 10 * MINUTE), "working, hub away");
    assert!(!keeps_awake(false, 10 * MINUTE), "nothing under way");
    assert!(!keeps_awake(true, Duration::ZERO), "its hub is connected");
    assert!(keeps_awake(true, 5 * 60 * MINUTE));
    assert!(
        !keeps_awake(true, 6 * 60 * MINUTE),
        "a hub gone that long leaves the machine to boxd's backstop"
    );
}

#[test]
fn a_node_counts_its_hub_away_until_a_hub_socket_opens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    assert!(!state.mesh.hub_away().is_zero());
}
