//! Top-level facade that owns the store, retriever, and write pipeline.

use crate::error::MemoryError;
use crate::refinement::{LlmRefiner, RefinerConfig};
use crate::retrieval::{MemoryRetriever, RetrievalConfig};
use crate::store::MemoryStore;
use crate::types::MemoryPacket;
use crate::write_pipeline::{MemoryWritePipeline, WriteConfig, WriteReport};
use aura_agent::AgentLoopResult;
use aura_core::AgentId;
use aura_reasoner::ModelProvider;
use rocksdb::{DBWithThreadMode, MultiThreaded};
use std::sync::Arc;

pub struct MemoryManager {
    store: Arc<MemoryStore>,
    retriever: MemoryRetriever,
    pipeline: MemoryWritePipeline,
}

impl MemoryManager {
    /// Create a new `MemoryManager` backed by a shared `RocksDB` instance.
    pub fn new(
        db: Arc<DBWithThreadMode<MultiThreaded>>,
        provider: Arc<dyn ModelProvider>,
        refiner_config: RefinerConfig,
        write_config: WriteConfig,
        retrieval_config: RetrievalConfig,
    ) -> Self {
        let store = Arc::new(MemoryStore::new(db));
        let retriever = MemoryRetriever::new(Arc::clone(&store), retrieval_config);
        let refiner = LlmRefiner::new(provider, refiner_config);
        let pipeline = MemoryWritePipeline::new(Arc::clone(&store), refiner, write_config);

        Self {
            store,
            retriever,
            pipeline,
        }
    }

    /// Retrieve a memory packet for system prompt injection.
    ///
    /// # Errors
    /// Returns error on store read failure.
    pub fn retrieve(&self, agent_id: AgentId) -> Result<MemoryPacket, MemoryError> {
        self.retriever.retrieve(agent_id)
    }

    /// Ingest an agent loop result through the write pipeline.
    ///
    /// # Errors
    /// Returns error on extraction, refinement, or storage failure.
    pub async fn ingest(
        &self,
        agent_id: AgentId,
        result: &AgentLoopResult,
    ) -> Result<WriteReport, MemoryError> {
        self.pipeline.ingest(agent_id, result).await
    }

    /// Inject agent memory into the system prompt of an `AgentLoopConfig`.
    ///
    /// Called before the agent loop starts a turn. Strips any existing
    /// `<agent_memory>` block to ensure idempotency, then appends a fresh one.
    pub fn prepare_context(
        &self,
        agent_id: AgentId,
        config: &mut aura_agent::AgentLoopConfig,
    ) {
        if let Some(idx) = config.system_prompt.find("\n<agent_memory>") {
            config.system_prompt.truncate(idx);
        }

        match self.retrieve(agent_id) {
            Ok(packet) => {
                let block = packet.format_for_prompt();
                if !block.is_empty() {
                    config.system_prompt.push_str(&block);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to retrieve memory for prompt injection");
            }
        }
    }

    /// Process an agent loop result through the two-stage write pipeline.
    ///
    /// Called after the agent loop completes a turn.
    ///
    /// # Errors
    /// Returns error on extraction, refinement, or storage failure.
    pub async fn process_result(
        &self,
        agent_id: AgentId,
        result: &AgentLoopResult,
    ) -> Result<WriteReport, MemoryError> {
        self.ingest(agent_id, result).await
    }

    /// Get a reference to the underlying memory store.
    #[must_use]
    pub const fn store(&self) -> &Arc<MemoryStore> {
        &self.store
    }
}
