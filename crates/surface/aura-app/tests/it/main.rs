//! aura-app's integration tests. They share one binary, since each test binary
//! links the crate and all of its dependencies again; add a test file here as a
//! module, not as a file of its own in `tests/`.

mod agent_list_perf;
mod chat_load_perf;
mod cloud_live;
mod startup_perf;
