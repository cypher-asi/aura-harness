//! Per-agent memory system for Aura.
//!
//! Provides fact storage, episodic event logging, procedural pattern detection,
//! a two-stage write pipeline (heuristic extraction + LLM refinement), and
//! deterministic retrieval for system prompt injection.

#![forbid(unsafe_code)]
#![warn(clippy::all)]
#![allow(clippy::option_if_let_else)]

mod consolidation;
mod error;
mod extraction;
mod manager;
mod procedures;
mod refinement;
mod retrieval;
mod salience;
mod store;
mod types;
mod write_pipeline;

#[cfg(test)]
mod test_kernel;

pub use consolidation::{ConsolidationConfig, ConsolidationReport, MemoryConsolidator};
pub use error::MemoryError;
pub use extraction::ConversationTurn;
pub use manager::MemoryManager;
pub use procedures::{compute_skill_relevance, ProcedureConfig, ProcedureExtractor, StepSequence};
pub use refinement::RefinerConfig;
pub use retrieval::{MemoryRetriever, RetrievalConfig};
pub use salience::{estimate_tokens, score_event, score_fact, score_procedure};
pub use store::{MemoryStats, MemoryStore, MemoryStoreApi};
pub use types::{AgentEvent, Fact, FactSource, MemoryCandidate, MemoryPacket, Procedure};
pub use write_pipeline::{MemoryWritePipeline, WriteConfig, WriteReport};
