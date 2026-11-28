//! aura-node's integration tests. They share one binary, since each test binary
//! links the crate and all of its dependencies again; add a test file here as a
//! module, not as a file of its own in `tests/`.

mod generation_proxy_guard;
mod hook_permission_request_short_circuits;
mod hook_pre_tool_use_blocks_mid_flight;
mod memory_observer_adapter;
mod plugin_e2e;
mod plugin_no_op_empty;
mod worker_routing_identity;
