//! aura-store-db's integration tests. They share one binary, since each test
//! binary links the crate and all of its dependencies again; add a test file
//! here as a module, not as a file of its own in `tests/`.

mod invariant_atomicity;
mod invariant_readstore_surface;
mod record_log;
mod replica_batch;
