//! aura-plugin-hooks's integration tests. They share one binary, since each
//! test binary links the crate and all of its dependencies again; add a test
//! file here as a module, not as a file of its own in `tests/`.

mod hook_event_smoke;
mod sandbox_env_scrubbing;
