//! aura-exec-merge-queue's integration tests. They share one binary, since each
//! test binary links the crate and all of its dependencies again; add a test
//! file here as a module, not as a file of its own in `tests/`.

mod broken_target;
mod conflicts;
mod fetch_refs;
mod git_integration;
mod overlap;
mod pull_requests;
mod support;
mod target_and_worktree;
