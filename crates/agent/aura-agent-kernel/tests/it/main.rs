//! aura-agent-kernel's integration tests. They share one binary, since each
//! test binary links the crate and all of its dependencies again; add a test
//! file here as a module, not as a file of its own in `tests/`.

mod invariant_determinism;
mod replay_round_trip;
