//! Phase 6c regression test — ensure a [`TurnSummary`] roundtrips through
//! the memory write pipeline with the same candidate count + write-report
//! shape that `AgentLoopResult` produced before the inversion.
//!
//! The test uses an in-memory `MemoryStoreApi` fake and a `MockProvider`
//! that returns an empty string (so the heuristic stage produces all the
//! candidates and the LLM refiner short-circuits without proposing
//! KEEP/DROP edits). This is intentional: pinning the heuristic-only
//! behaviour catches any future change that drops or duplicates a
//! candidate purely because of the `TurnSummary` swap.
//!
//! The harness drives [`MemoryWritePipeline`] directly because
//! [`MemoryManager::new`] takes a live RocksDB handle (per the
//! production contract). The pipeline is what `MemoryManager::ingest`
//! delegates to, so the assertion still pins the ingest path.

#![allow(clippy::expect_used)]

mod ingest;
mod safety;
mod shared_conflicts;
mod support;
