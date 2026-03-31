//! Kernel gateway adapters that bridge the new Kernel API to existing AgentLoop traits.
//!
//! - [`KernelToolGateway`] implements [`AgentToolExecutor`] by routing tool calls
//!   through [`Kernel::process_tools`].
//! - [`KernelModelGateway`] implements [`ModelProvider`] by routing completions
//!   through [`Kernel::reason`] and [`Kernel::reason_streaming`].

use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use async_trait::async_trait;
use aura_kernel::Kernel;
use aura_reasoner::{ModelProvider, ModelRequest, ModelResponse, ReasonerError, StreamEventStream};
use std::sync::Arc;
use tracing::warn;

// ============================================================================
// KernelToolGateway
// ============================================================================

/// Routes [`AgentToolExecutor::execute`] through the kernel's batch tool processor.
///
/// Converts `ToolCallInfo` slices into `ToolProposal` vectors, delegates to
/// `Kernel::process_tools`, and maps the results back to `ToolCallResult`.
pub struct KernelToolGateway {
    kernel: Arc<Kernel>,
}

impl KernelToolGateway {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

#[async_trait]
impl AgentToolExecutor for KernelToolGateway {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let proposals: Vec<aura_core::ToolProposal> = tool_calls
            .iter()
            .map(|tc| aura_core::ToolProposal::new(&tc.id, &tc.name, tc.input.clone()))
            .collect();

        match self.kernel.process_tools(proposals).await {
            Ok(results) => results
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    if let Some(output) = r.tool_output {
                        ToolCallResult {
                            tool_use_id: output.tool_use_id,
                            content: output.content,
                            is_error: output.is_error,
                            stop_loop: false,
                        }
                    } else {
                        let tc = &tool_calls[i];
                        ToolCallResult::error(&tc.id, "No output from kernel")
                    }
                })
                .collect(),
            Err(e) => {
                warn!(error = %e, "Kernel process_tools failed");
                tool_calls
                    .iter()
                    .map(|tc| ToolCallResult::error(&tc.id, format!("Kernel error: {e}")))
                    .collect()
            }
        }
    }
}

// ============================================================================
// KernelModelGateway
// ============================================================================

/// Routes [`ModelProvider`] calls through the kernel's reasoning layer,
/// ensuring all model interactions are recorded in the append-only log.
pub struct KernelModelGateway {
    kernel: Arc<Kernel>,
}

impl KernelModelGateway {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

#[async_trait]
impl ModelProvider for KernelModelGateway {
    fn name(&self) -> &'static str {
        "kernel-gateway"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let result = self
            .kernel
            .reason(request)
            .await
            .map_err(|e| ReasonerError::Internal(format!("kernel reason error: {e}")))?;
        Ok(result.response)
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let (handle, stream) = self
            .kernel
            .reason_streaming(request)
            .await
            .map_err(|e| ReasonerError::Internal(format!("kernel reason_streaming error: {e}")))?;

        // TODO: In Phase 5+, wrap the stream to auto-record via the handle.
        drop(handle);

        Ok(stream)
    }

    async fn health_check(&self) -> bool {
        true
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::AgentId;
    use aura_kernel::{ExecutorRouter, KernelConfig};
    use aura_reasoner::{Message, MockProvider};
    use aura_store::RocksStore;
    use tempfile::TempDir;

    fn create_test_kernel() -> (Arc<Kernel>, TempDir, TempDir) {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();
        let store: Arc<dyn aura_store::Store> =
            Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("gateway test response"));
        let executor = ExecutorRouter::new();
        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..KernelConfig::default()
        };
        let kernel = Arc::new(Kernel::new(store, provider, executor, config, agent_id).unwrap());
        (kernel, db_dir, ws_dir)
    }

    #[tokio::test]
    async fn test_model_gateway_complete() {
        let (kernel, _db, _ws) = create_test_kernel();
        let gateway = KernelModelGateway::new(kernel);

        let request = ModelRequest::builder("test-model", "system")
            .message(Message::user("hello"))
            .build();
        let response = gateway.complete(request).await.unwrap();
        assert!(!response.message.content.is_empty());
    }

    #[tokio::test]
    async fn test_model_gateway_name() {
        let (kernel, _db, _ws) = create_test_kernel();
        let gateway = KernelModelGateway::new(kernel);
        assert_eq!(gateway.name(), "kernel-gateway");
    }

    #[tokio::test]
    async fn test_tool_gateway_empty_batch() {
        let (kernel, _db, _ws) = create_test_kernel();
        let gateway = KernelToolGateway::new(kernel);
        let results = gateway.execute(&[]).await;
        assert!(results.is_empty());
    }
}
